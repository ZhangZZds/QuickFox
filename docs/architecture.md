# QuickFox 架构说明

## 总览

QuickFox 采用 Tauri 双端架构：

- 前端负责窗口 UI、输入交互、结果渲染和设置表单
- Rust core 负责查询解析、索引、Provider、Action、配置、历史和平台适配

## 模块边界

### 前端

- `src/App.tsx`
  - 启动器主界面
  - 搜索输入、结果列表、命令预览、设置页
- `src/tauriClient.ts`
  - 前端与 Tauri commands 的边界

### Rust core

- `src-tauri/src/core/search.rs`
  - 查询解析
  - 结果模型
  - 排序器
- `src-tauri/src/core/index.rs`
  - name/path 与正文查询入口
  - 索引状态模型、模糊搜索与正则搜索
- `src-tauri/src/core/compact_index.rs`
  - packed entry table 与有界 name/path 候选检索
  - 1–3 字符 name n-gram 的 delta-varint posting、路径段索引与碰撞复核
- `src-tauri/src/core/index_watcher.rs`
  - 平台 watcher adapter 与 8192 容量的非阻塞事件通道
- `src-tauri/src/core/index_update_coordinator.rs`
  - 5 秒静默窗口、10 秒硬上限、事件折叠与 dirty-root 协调
- `src-tauri/src/core/targeted_index_scanner.rs`
  - 单路径/局部子树扫描与 directory manifest 校准
- `src-tauri/src/core/layered_index.rs`
  - compact baseline、delta overlay、tombstone 与 generation view
- `src-tauri/src/core/index_journal.rs`
  - journal 提交、幂等恢复与 manifest repository 边界
- `src-tauri/src/core/index_refresh_orchestrator.rs`
  - 启动/配置 revision/全量刷新时的 capture、校准与 generation fence
- `src-tauri/src/core/index_generation.rs`
  - `building`、`prepared`、`active`、`obsolete` 代际协议、恢复判定和存储诊断
- `src-tauri/src/core/index_source.rs`
  - 流式全量索引来源、进度、取消和目录断点接口
- `src-tauri/src/core/generic_index_source.rs`
  - 通用文件系统扫描降级路径
- `src-tauri/src/core/windows_ntfs_index_source.rs`
  - Windows 固定 NTFS 卷能力探测和无服务 Win32 批量枚举；不静默提权
- `src-tauri/src/core/runtime_indexing.rs`
  - watcher、coordinator、scanner、journal 和 layered view 的运行期服务
- `src-tauri/src/core/content_index.rs`
  - Tantivy 正文索引、增量正文更新与版本目录生命周期
- `src-tauri/src/core/providers.rs`
  - FileProvider
  - CalculatorProvider
  - WebSearchProvider
  - CommandProvider
- `src-tauri/src/core/actions.rs`
  - 统一 Action 模型
  - Action dispatcher
- `src-tauri/src/core/platform.rs`
  - 路径适配
  - 打开行为抽象
  - 终端命令构造
  - 命令安全检查
- `src-tauri/src/core/config.rs`
  - TOML 配置模型与默认配置
- `src-tauri/src/core/storage.rs`
  - SQLite 持久化、历史与索引快照
  - baseline 分块事务、完成状态、旧批次回收与磁盘空间预算

### Tauri 集成层

- `src-tauri/src/lib.rs`
  - 应用运行时状态
  - Tauri commands 暴露
  - 菜单栏图标与窗口显示逻辑
  - 桌面动作执行

## 数据流

```text
前端输入
  -> tauriClient.search()
  -> Rust QueryParser
  -> ProviderRegistry
  -> SearchIndex / Calculator / Web / Command
  -> Ranker
  -> SearchResult[]
  -> 前端列表渲染
  -> 用户触发动作
  -> tauriClient.executeAction()
  -> Rust execute_action
  -> 平台打开 / 剪贴板 / 外部终端
```

## Provider / Action / Adapter 关系

- Provider 只负责“产出结果”，不直接执行系统行为
- SearchResult 只携带标准化 Action
- Action 执行统一收口到 Rust
- 平台差异隔离在 Adapter 或平台命令构造中

## 运行时状态

Tauri 启动时会初始化一份运行时状态，包含：

- 当前配置
- 当前内存索引
- 最近一次索引报告
- 当前索引状态

启动同步路径只创建单实例、托盘、快捷键和空的最小 Runtime。SQLite schema、最近 active baseline 和 committed journal 在 Tauri setup 返回后的后台 worker 中恢复，完成后通过短临界区挂载到查询视图。第二实例因此不需要加载完整 SQLite。恢复期间应用、历史、计算器、网页搜索和命令 Provider 可用；没有 baseline 时文件 Provider 返回暂不可用反馈。

搜索和刷新索引都基于这份状态工作，而不是每次由前端硬编码构造假数据。计算器、网页搜索和命令 Provider 不依赖文件索引，因此索引构建期间仍可用。

### 自动增量流程

自动增量默认开启。普通文件变化依次经过：

```text
平台 watcher
  -> 8192 容量非阻塞 channel
  -> IndexUpdateCoordinator（5 秒静默 / 10 秒硬上限）
  -> TargetedIndexScanner（单路径或局部子树）
  -> SQLite committed journal + manifest mutation
  -> LayeredSearchIndex（baseline + overlay - tombstone）
  -> name/path 状态发布
  -> 容量 8 的正文增量队列与单 worker
  -> ContentIndex 原子发布
```

- create/write 读取受影响文件；新目录只扫描该子树。
- remove 不重新访问已删除路径，直接生成文件或子树 tombstone。
- rename 折叠为旧路径 tombstone 加新路径 targeted scan。
- 相同路径的重复事件在批次内按最终文件系统语义折叠。
- SQLite 先事务提交 journal，内存 generation 后切换；崩溃后只重放 committed batch，重复重放按规范化路径保持幂等。
- name/path 批次先完成并可搜索，正文更新随后独立执行；正文失败不会回滚已成功的 name/path 更新。

普通事件在最后一个事件后静默 5 秒提交；持续事件流从首个事件起最迟 10 秒强制提交。10 秒门槛从 watcher 把事件交付给 coordinator 时开始计算，不包含操作系统尚未交付事件的时间。

### 手动增量与 dirty-root 恢复

手动“刷新索引”默认不是全盘 walk。系统先提交待处理 watcher 事件，再用持久化 directory manifest 校准 dirty 或变化目录：

1. 对已知目录读取轻量指纹；未变化目录不执行 `read_dir`，但继续检查已知子目录。
2. 指纹变化的目录只枚举直接子项并与 manifest 比较。
3. 新目录递归建立清单；缺失目录生成子树删除。
4. 某个目录不可读时继续处理其他目录，且不把未确认路径误判为删除。

平台 overflow、channel 满或 journal 写入失败会把能识别的 root 标为 dirty，优先走同一校准流程。校准无法建立可信差异、持久化状态损坏或内容级一致性无法确认时，才带原因升级为后台全量刷新。刷新期间最近可用 baseline 与当前增量视图继续服务。

### 分层查询视图

运行期文件搜索由三层组成：

- `CompactBaseline`：最近完成并激活的不可变主索引。
- `DeltaOverlay`：以规范化路径为键的新增/修改条目。
- `TombstoneSet`：精确删除与目录前缀删除。

baseline 在候选截断前过滤已被 overlay 替换或 tombstone 屏蔽的条目，overlay 独立查询，最后沿用既有 matcher/ranker 合并、去重和排序。单次查询捕获同一 generation 的不可变 view，因此不会混读切换前后的状态。普通小批次只重建小型 overlay 候选结构，不重建百万级 baseline。

baseline 的 name 候选使用 1–3 字符 n-gram：每个 posting 的有序 `EntryId` 使用 delta-varint 压缩。短查询和数字子串也由该索引生成候选，而不是退化为遍历整个 entry table；命中 fingerprint 后仍回读 packed name 复核，避免 hash 碰撞改变结果。路径 fuzzy 先按首字符和必要 ASCII 字符集合缩小范围，最终仍交给 matcher 验证 subsequence 语义。

全量刷新使用明确代际：扫描写入 `building` 并逐目录保存断点，完成扫描后转为 `prepared`，最终索引与 manifest 就绪后在单个事务中切换为 `active`，旧 active 转为 `obsolete`。启动遇到同配置 `building` 时从目录前沿续扫，遇到 `prepared` 时直接恢复 Preview 并继续 finalization，不创建新扫描代际。搜索视图始终是 `ActiveBaseline + RootPreviews + IncrementalOverlay`；只有新 active 安装成功后才释放 Preview，finalizing 失败时旧 active 和 Preview 继续服务。

Windows 固定 NTFS 卷优先使用 `FindFirstFileExW(FindExInfoBasic, FIND_FIRST_EX_LARGE_FETCH)` 流式枚举，并保留每目录恢复断点。Adapter 会探测卷类型和 USN Journal，但当前不会读取原始 MFT，也不会安装服务或触发提权；权限、文件系统或语义不适用时回退 Generic Scanner。MFT/USN 高权限服务仍需独立安全设计和维护者批准。

正文查询复用相同可见性语义，但查询时不遍历 baseline 构造隐藏路径。delta 提交时生成只含 overlay 路径和 tombstone 的不可变可见性快照，Tantivy 命中后按路径段过滤。没有正文索引时，普通 name/path 查询仍可用；`content:` 返回“内容索引仍在准备”。非法正文查询返回语法反馈；reader/search I/O 失败返回“内容索引查询失败”，并让运行时进入可观察降级与恢复。

### 配置 desired/applied revision

仅切换 `watcher_enabled` 不重建 baseline：关闭会停止新事件消费但保留 baseline、journal 和搜索结果；重新开启会先校准全部 root，成功后才进入 `Watching`。

include/exclude roots、exclude patterns、性能模式、project ignore 或正文索引范围属于索引语义。保存这类配置时，配置提交和索引应用是两个可观察阶段：

1. 校验通过后先持久化 desired config，并递增 desired revision；保存命令不执行目录遍历，也不等待 watcher、baseline 或内容索引。
2. 最近可用查询视图继续服务，新 revision 在后台建立 active roots、capture、baseline 和 service；完成后更新 applied revision。
3. 设置页分别显示 `applying`、`applied`、`partial` 或 `failed`。后台失败不能回滚已经持久化的 desired config；用户重开设置仍看到最后保存值。

`fast`、`balanced`、`complete` 先生成唯一 active-root 集合，baseline、standby/runtime watcher 和 calibration 都使用该集合；`fast` 不会因为配置中仍保留 `D:\` 而重新扫描整盘。新 desired revision 会在 walker 条目边界取消旧 revision。单个 root 不可访问时，系统提交其他可用 root 的结果并标记 `partial`/dirty；缺失盘符恢复后由 root monitor 触发重试。全量 baseline 激活仍会先 drain durable successor，把 entries 与 manifest materialize 到同一权威 generation，防止刷新交接窗口丢事件。

Windows 首次配置默认把当前可用的盘符根目录写入 `include_dirs`；未发现盘符时才回退当前用户 profile。默认仍为 `balanced`，应用入口和用户热路径先提供结果，全盘范围在后台补全。baseline、watcher 与 calibration 统一排除 `Windows`、`ProgramData`、`PerfLogs`、恢复/升级目录、卷元数据、回收站、`AppData` 和虚拟内存文件等系统噪音，但不排除 `Users`、Desktop 或 Documents。仅完整匹配 v1.6.1 自动热路径默认指纹的配置会迁移为盘符范围，用户修改过任一索引字段时保持原值。

### 有界资源与降级

| 资源                         | 上限/策略                      | 达到上限后的行为                       |
| ---------------------------- | ------------------------------ | -------------------------------------- |
| watcher channel              | 8192 个标准化事件              | 非阻塞拒绝，root 标 dirty              |
| coordinator pending 唯一路径 | 8192 条                        | 清逐项追赶，转 dirty-root 校准         |
| debounce                     | 5 秒静默，首事件后 10 秒硬上限 | 强制形成批次                           |
| overlay + tombstone          | 50,000 条                      | 安排后台 baseline 全量刷新             |
| 估算增量状态                 | 64 MiB                         | 安排后台 baseline 全量刷新             |
| 单 baseline 估算存储         | 8 GiB                          | 拒绝持久化并提示缩小索引目录           |
| 索引存储可用空间             | 至少保留 5 GiB                 | 拒绝新 baseline，保留最近可用索引      |
| 正文增量队列                 | 8 个 job，单 worker 串行发布   | 不阻塞 name/path；安排可恢复的后台刷新 |
| 单文件正文                   | 默认最大 2 MiB                 | 过大/二进制/不支持文件只跳过正文       |

结构化 degradation code 为：`watcherInitializationFailed`、`watcherRuntimeFailed`、`watcherOverflow`、`channelOverflow`、`journalWriteFailed`、`journalReplayFailed`、`calibrationFailed`、`fullRefreshFallback`。前端同时只接收 enabled/state、pending 数、dirty root 数、最近批次条目/耗时和 code；不得包含完整路径或原始 watcher 事件。

索引状态另外提供 `phase`、`refreshReason` 和缓存的 SQLite 诊断。诊断包括 active/pending 代际、各代际条目数、主库/WAL 大小、freelist/page 数、auto-vacuum 迁移状态和最近 GC；采集与维护在后台执行，状态查询本身不打开数据库。

### 正文索引版本目录与 GC

每次正文 build 都在 `content-v1` 下创建随机 `build-*` 独立目录。新 reader 原子发布，旧目录由 lease 保持到最后一个 reader 释放后再回收，避免覆盖仍在查询的 Tantivy 文件。

进程内 active registry 和 marker 跨进程文件锁共同保护活跃版本。启动和下一次 build 只 GC 同时满足以下条件的 crash orphan：名称受管、是真实目录、包含内容完全匹配的真实普通 marker 文件、当前进程未登记且能非阻塞取得 marker 锁。GC 不跟随 symlink，不删除未标记目录、用户文件或另一 QuickFox 进程仍在使用的版本。

### 隐私边界

- 索引、journal、manifest 和正文版本目录都保存在本机，不接入外部索引服务。
- 用户可见状态、失败摘要、测试快照和遥测字段只包含计数、耗时、阶段、generation 与结构化 code，不包含完整路径和原始事件。
- 设置页可在用户主动查看时显示配置文件和索引存储位置；这不等于把路径写入状态广播或故障摘要。
- 开发日志若为诊断必须避免 token、密钥等敏感值；提交 issue/截图前应遮盖用户名、盘符标签和项目路径。

## 当前已知限制

- 设置页已支持索引规则、网页搜索引擎、历史和命令安全等核心配置；更细的外观配置仍较轻量
- 命令执行走外部终端，强确认 UI 还没有单独展开
- 历史持久化与排序权重已经有 core 边界，但前端还未完整可视化
