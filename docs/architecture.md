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

启动时优先从 SQLite 读取最近完成的 baseline，再按 generation 重放 committed journal。恢复出的 name/path 视图会立即提供查询；没有 baseline 时文件 Provider 不阻塞其他 Provider，并返回文件索引暂不可用的反馈。启动校准和正文索引构建均在 Tauri setup 返回后的后台 worker 中进行。

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

正文查询复用相同可见性语义，但查询时不遍历 baseline 构造隐藏路径。delta 提交时生成只含 overlay 路径和 tombstone 的不可变可见性快照，Tantivy 命中后按路径段过滤。没有正文索引时，普通 name/path 查询仍可用；`content:` 返回“内容索引仍在准备”。非法正文查询返回语法反馈；reader/search I/O 失败返回“内容索引查询失败”，并让运行时进入可观察降级与恢复。

### 配置 revision 的两阶段切换

仅切换 `watcher_enabled` 不重建 baseline：关闭会停止新事件消费但保留 baseline、journal 和搜索结果；重新开启会先校准全部 root，成功后才进入 `Watching`。

include/exclude roots、exclude patterns、project ignore 或正文索引范围属于索引语义。保存这类配置时使用两阶段切换：

1. 旧配置、旧 service 和旧查询视图继续工作；新 revision 建立 candidate watcher capture。
2. candidate 完成校准并越过 generation/storage fence 后，才持久化配置并原子替换内存配置、索引视图和 service。

任一 watcher、scanner、storage、配置持久化或 dispatch 步骤失败都会丢弃 candidate，保留旧配置/service/view，并进入统一的 `Degraded` 恢复路径。全量 baseline 激活也会先 drain durable successor，把 entries 与 manifest materialize 到同一权威 generation，防止刷新交接窗口丢事件。

### 有界资源与降级

| 资源                         | 上限/策略                      | 达到上限后的行为                       |
| ---------------------------- | ------------------------------ | -------------------------------------- |
| watcher channel              | 8192 个标准化事件              | 非阻塞拒绝，root 标 dirty              |
| coordinator pending 唯一路径 | 8192 条                        | 清逐项追赶，转 dirty-root 校准         |
| debounce                     | 5 秒静默，首事件后 10 秒硬上限 | 强制形成批次                           |
| overlay + tombstone          | 50,000 条                      | 安排后台 baseline 全量刷新             |
| 估算增量状态                 | 64 MiB                         | 安排后台 baseline 全量刷新             |
| 正文增量队列                 | 8 个 job，单 worker 串行发布   | 不阻塞 name/path；安排可恢复的后台刷新 |
| 单文件正文                   | 默认最大 2 MiB                 | 过大/二进制/不支持文件只跳过正文       |

结构化 degradation code 为：`watcherInitializationFailed`、`watcherRuntimeFailed`、`watcherOverflow`、`channelOverflow`、`journalWriteFailed`、`journalReplayFailed`、`calibrationFailed`、`fullRefreshFallback`。前端同时只接收 enabled/state、pending 数、dirty root 数、最近批次条目/耗时和 code；不得包含完整路径或原始 watcher 事件。

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
