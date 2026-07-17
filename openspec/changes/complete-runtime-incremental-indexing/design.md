## Context

QuickFox v1.5.0 已将普通 name/path 查询切换到 compact candidate index，并建立 200 万 entry 查询性能门槛。运行期增量链路却仍停留在未完成状态：`RuntimeIndexWatcher` 创建后丢弃 receiver，`IndexEventBatcher` 和 `SearchIndex::apply_update_batch` 没有接入 runtime；后者即使被调用，也会为每个 batch 重建完整 compact index。SQLite 仍按完整 batch 保存所有 entry。

本变更必须保持 Tauri 边界：前端只配置与展示状态，Rust core 管理监听、扫描、查询、持久化和恢复，平台差异封装在 watcher adapter。macOS 与 Windows 是首轮真实桌面验收目标，Linux 继续运行 core/CI 测试并保留同一接口。

## Goals / Non-Goals

**Goals:**

- 自动增量默认开启，普通变化使用 5 秒静默窗口合并，并在事件到达后最迟 10 秒进入可搜索视图。
- 单文件事件只读取受影响路径；目录变化只枚举受影响子树；手动刷新通过持久化目录清单定位变化目录。
- 普通增量不重建百万级 baseline，以 baseline、overlay 和 tombstone 组成一致查询视图。
- journal 可事务提交、幂等重放；崩溃、watcher overflow 和权限失败后保留最近可用索引并能恢复。
- 队列、overlay、tombstone、journal 和状态摘要均有上限，并维持现有 200 万 entry 查询预算。

**Non-Goals:**

- 不实现统一结果动作与反馈；该能力属于后续独立变更。
- 不实现完整本地诊断面板，只提供本阶段需要的结构化状态 contract。
- 不接入系统或第三方搜索服务。
- 不改变查询语法、Provider 排序或命令安全边界。
- 不在本变更中优化 PDF/Office 内容抽取。

## Decisions

### 1. 单一 Index Update Coordinator 消费有界事件通道

`RuntimeIndexWatcher` 返回 watcher handle 与 receiver；`IndexUpdateCoordinator` 在独立线程持有 receiver、debounce queue、dirty roots 和取消标记。channel 容量固定为 8192 个标准化事件，callback 使用非阻塞发送，避免平台 watcher 线程被索引 IO 卡住。

普通事件在 5 秒无新事件后提交；持续事件流达到首个事件后 10 秒时强制提交，防止 debounce 永久延后。相同路径事件按 create/write/remove/rename 的最终语义折叠。

当 channel 满或平台报告 overflow 时，共享 overflow state 将能识别的 root 标记为 dirty；Coordinator 不再尝试逐项追赶，而是安排 root 校准。

备选是每个 callback 直接修改索引。该方案会把平台线程、SQLite 和 runtime lock 耦合，事件风暴时容易阻塞或死锁，因此拒绝。

### 2. Targeted Scanner 与目录清单共同定位变化

事件驱动路径使用 `TargetedIndexScanner`：

- 文件 create/write：读取该路径的类型、mtime、size 和包含/排除规则。
- 目录 create：扫描新目录子树。
- remove：不访问文件系统，生成 path/subtree tombstone。
- rename：旧路径 tombstone 加新路径 targeted scan。

手动增量刷新与 dirty-root 恢复使用持久化 directory manifest。manifest 保存每个已知目录的规范化路径、所属 root、mtime 和已知直接子目录关系。校准会 `stat` 已知目录；仅对 mtime 改变的目录执行 `read_dir` 并比较直接子项，新目录再递归扫描，缺失目录删除整个已知子树。这样成本与目录数相关，不重新读取和构造所有文件 entry。

普通文件内容写入通常不改变父目录 mtime，因此 watcher write 仍是内容索引更新的主要信号；overflow 后若需要内容级完全一致性，root 校准可升级为后台全量刷新并明确报告原因。

备选是依赖根目录 mtime 跳过整个子树。深层目录变化不会可靠更新所有祖先 mtime，会产生漏检，因此拒绝。另一备选是每次手动刷新重新 walk 全部文件，无法满足本变更的增量目标。

### 3. Baseline、Overlay、Tombstone 组成 LayeredSearchIndex

运行时保存：

- 不可变 `CompactBaseline`：最近完成快照构造的主索引。
- `DeltaOverlay`：按规范化 path 保存新增/修改 entry，并只为 overlay 重建小型候选结构。
- `TombstoneSet`：保存删除路径和删除目录前缀。

baseline 查询在候选评分前接受 visibility predicate，过滤已被 overlay 替换或 tombstone 屏蔽的 entry；overlay 独立查询。两路结果使用现有结果比较语义合并、去重和截断，保证类型、匹配质量与历史排序不漂移。一次查询捕获同一个 generation 的不可变 view，批次提交通过短锁或 `Arc` 原子替换完成。

备选是在每个 batch 后调用现有 `apply_update_batch`。它会重建完整 compact index，正是需要消除的 O(N) 成本。备选 SQLite 直接承载每次按键查询会改变 fuzzy 延迟与排序控制，也不采用。

### 4. SQLite baseline 继续使用完整 batch，新增可重放 journal 与 manifest

保留现有 `index_batches/index_entries` 作为 baseline 快照，增加：

- `index_delta_batches`：generation、状态、创建/提交时间。
- `index_delta_entries`：batch、operation、path、entry metadata；remove 行允许 metadata 为空。
- `index_directory_manifest`：baseline/delta 视图下的目录指纹和父子关系。
- `index_runtime_state`：active baseline、last applied journal generation、dirty/refresh 原因摘要。

批次提交顺序是：SQLite 事务写入 committed journal 和 manifest 变化，然后构建新 overlay/tombstone view，最后更新 runtime generation 并发出状态事件。启动加载最新 baseline 后按 generation 顺序重放 committed journal；重放以 path 为键且幂等。未 committed batch 被忽略。

旧 snapshot 仍可加载；若没有 manifest，文件搜索立即使用旧 baseline，后台建立 manifest。schema 不兼容或 journal 无法重放时保留 baseline，并启动明确标记原因的全量刷新。

备选是每个事件批次复制并保存完整 snapshot，会延续写放大和内存峰值，因此拒绝。

### 5. 安全阈值触发现有后台全量刷新

本阶段实现安全而非最终最优的 baseline 维护：overlay 与 tombstone 合计达到 50,000 条，或估算增量状态超过 64 MiB 时，Coordinator 标记 `baseline_refresh_required`，在没有其他 refresh 运行时启动现有后台全量刷新。旧 layered view 在刷新期间继续提供搜索；新 baseline 成功切换后清空已合并 journal、overlay 和 tombstone。

后续 `add-local-index-diagnostics-and-compaction` 变更再基于真实 benchmark 引入更精细的空闲期 SQL/compact compaction。当前安全阈值避免无界增长，又不把复杂 compaction 混入第一阶段。

### 6. 配置变化分为开关变化与索引语义变化

`watcher_enabled` 保持现有配置字段并默认 `true`。关闭时停止 watcher/coordinator 接收新事件，但不删除 baseline、journal 或搜索结果；重新开启时启动 watcher 并先校准所有 root。

include/exclude roots、exclude patterns、project ignore 或内容索引范围改变会使现有 manifest 语义失效，必须触发带原因的后台全量刷新。仅切换 watcher 开关不触发全量重建。

### 7. 结构化状态而非日志推断

`IndexStatus` 增加自动增量状态、pending event 数、dirty root 数、最近 batch 数量/耗时和 degradation code。路径和原始事件不进入前端状态。状态更新复用现有 Tauri event，并继续遵守前端防抖，避免事件 batch 形成搜索风暴。

错误 code 至少区分 watcher 初始化失败、watcher 运行失败、运行期 overflow、channel overflow、journal 写入失败、journal 重放失败、校准失败和 full refresh fallback。日志可包含开发诊断，但用户可见恢复不能依赖解析日志。

## Risks / Trade-offs

- [Risk] layered 查询过滤时机不正确会漏掉 baseline 后续候选。→ 在候选评分/截断前应用 visibility predicate，并用旧完整索引 oracle 覆盖 overlay、删除和排序 fixture。
- [Risk] directory manifest 需要对已知目录执行 metadata 检查，大目录数场景仍有成本。→ 校准在后台运行、按 root 分批、支持取消，并记录目录数和耗时；普通低延迟路径仍由 watcher 驱动。
- [Risk] watcher 在 macOS/Windows 上产生不同 rename 或重复事件。→ Adapter 归一化后再由 batcher 折叠，以最终文件系统状态为准，真实平台验收覆盖 rename、批量 checkout 和休眠唤醒。
- [Risk] SQLite journal 与内存 view 切换之间崩溃。→ journal 先 committed，内存后切换；重启幂等重放。未 committed 数据不参与恢复。
- [Risk] 50,000 条/64 MiB 阈值可能对低配机器偏高。→ 两个阈值均有自动化内存测试，后续诊断/compaction 变更可在不改变 contract 的情况下调整。
- [Risk] 全量刷新 fallback 在事件风暴后产生 IO。→ 保持旧索引可用、后台执行、状态说明原因，并优先尝试 dirty-root 校准。

## Migration Plan

1. 新 schema 以向前兼容 migration 创建 journal、manifest 和 runtime state 表，不修改旧 baseline 数据。
2. 升级后先加载旧 baseline；若 manifest 缺失，状态显示正在准备自动增量，同时后台建立 manifest。
3. manifest 准备完成后启动 watcher/coordinator；准备期间手动刷新沿用后台全量刷新，并说明原因。
4. 新 journal 成功运行后，后续普通变化进入 layered view；旧 batch 继续作为 rollback baseline。
5. 若新链路初始化失败，停止自动增量但保留 baseline 搜索和手动后台全量刷新。
6. 回滚旧版本时，旧代码忽略新增表并继续读取最近完整 baseline；不会依赖 journal 才能启动。

## Open Questions

无阻塞问题。50,000 条与 64 MiB 是本阶段安全上限，实施中只能通过有记录的性能测试收紧，不得在没有 OpenSpec 更新的情况下放宽。
