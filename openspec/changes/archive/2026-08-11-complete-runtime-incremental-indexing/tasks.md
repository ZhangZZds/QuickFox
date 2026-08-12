## 1. 回归基线与模块边界

- [x] 1.1 增加失败测试，证明当前 watcher receiver 被丢弃后创建事件不会更新运行期搜索结果
- [x] 1.2 增加失败性能测试，证明现有 `apply_update_batch` 会重建完整 compact candidate index
- [x] 1.3 建立 `index_update_coordinator`、`targeted_index_scanner`、`layered_index` 和 `index_journal` 模块边界，保持 `lib.rs` 只负责 Tauri/runtime 接线

## 2. 有界 watcher 事件协调器

- [x] 2.1 按 TDD 让 `RuntimeIndexWatcher` 返回可持续消费的 receiver，并支持 8192 容量的非阻塞事件通道
- [x] 2.2 按 TDD 实现 5 秒静默窗口、10 秒硬上限和 create/write/remove/rename 的确定性批次折叠
- [x] 2.3 按 TDD 实现 channel overflow、平台 overflow 和 watcher failure 的 dirty-root 标记与结构化失败 code
- [x] 2.4 实现 coordinator 生命周期、取消和关闭，验证保存配置、刷新和应用退出不会泄漏 watcher 线程

## 3. Targeted scanner 与目录清单

- [x] 3.1 按 TDD 实现单文件 create/write、remove 和 rename 的 targeted scan 操作，并复用现有包含/排除规则
- [x] 3.2 按 TDD 实现新建目录的局部子树扫描以及删除目录的 subtree tombstone
- [x] 3.3 为 SQLite 增加 directory manifest schema、迁移和 repository API，保存目录指纹及父子关系
- [x] 3.4 按 TDD 实现 manifest 校准：检查已知目录指纹，只枚举变化目录，并生成新增、变化和缺失 delta
- [x] 3.5 验证部分权限失败保留未确认删除的 baseline entry，并继续校准其他目录

## 4. Baseline、Overlay 与 Tombstone 查询

- [x] 4.1 按 TDD 实现规范化路径键、overlay 替换和目录前缀 tombstone 数据结构
- [x] 4.2 扩展 compact baseline 查询，在候选评分和截断前应用 visibility predicate
- [x] 4.3 按 TDD 实现 `LayeredSearchIndex` 的 baseline/overlay 合并、去重、稳定排序与 generation snapshot
- [x] 4.4 使用现有完整索引作为 oracle，验证新增、覆盖、文件删除、目录删除、重命名和历史排序语义
- [x] 4.5 验证普通增量只重建 overlay 候选结构，compact baseline build count 不发生变化

## 5. Delta journal、恢复与基线安全阈值

- [x] 5.1 为 SQLite 增加 delta batch/entry 和 runtime state schema，保持旧 `index_batches/index_entries` baseline 可读
- [x] 5.2 按 TDD 实现 journal 事务提交、未提交批次忽略、generation 顺序和 path-key 幂等重放
- [x] 5.3 按 TDD 实现启动加载 baseline 后重放 journal，并在 journal 损坏时保留 baseline、记录 fallback 原因
- [x] 5.4 实现“journal 先提交、内存 generation 后切换”的 runtime commit，并覆盖两个边界之间崩溃的恢复测试
- [x] 5.5 实现 overlay+tombstone 达到 50,000 条或 64 MiB 时安排现有后台全量刷新，成功后清理已合并增量状态

## 6. Runtime、配置与状态接线

- [x] 6.1 将 coordinator 接入 `QuickFoxRuntime`，在 baseline/manifest 准备完成后启动 watcher 并把批次提交到 layered view
- [x] 6.2 让自动增量配置默认开启；关闭时停止 watcher 但保留搜索，重新开启时先执行 root 校准
- [x] 6.3 区分 watcher 开关变化与索引语义变化，后者触发带原因的后台全量重建
- [x] 6.4 扩展 `IndexStatus` 和 Tauri contract，暴露自动增量状态、pending/dirty 数、最近批次摘要和 degradation code，不暴露完整路径
- [x] 6.5 更新前端设置页的开关与基础状态文案，并验证索引状态事件继续走防抖、不会制造查询风暴
- [x] 6.6 将 name/path 增量与内容索引更新解耦，验证内容更新失败不回滚已成功的 name/path batch

## 7. 性能、事件风暴与恢复验证

- [x] 7.1 增加 2,000,000 baseline 加 10,000 overlay/tombstone 的 ignored release benchmark，验证查询 P95 小于等于 50ms
- [x] 7.2 增加增量 batch benchmark，输出条目数、提交耗时、分层数量、查询延迟和估算内存
- [x] 7.3 增加事件风暴测试，证明 channel 不超过 8192、root 会标 dirty 且最近可用搜索视图保持可用
- [x] 7.4 增加旧快照迁移、重复 journal 重放、损坏 journal、权限失败、取消和 baseline refresh 失败的集成测试
- [x] 7.5 更新 macOS 与 Windows 手工 QA，记录普通变化耗时、批量 Git checkout、休眠唤醒、root 断开和 watcher 失败恢复（真实桌面项已明确为发布 blocker）

## 8. 文档、验证与交付

- [x] 8.1 更新架构、索引性能和 troubleshooting 文档，说明自动/手动增量、fallback、资源上限和隐私边界
- [x] 8.2 运行 Rust 定向测试、`cargo fmt --check`、`cargo clippy -- -D warnings`、前端测试和 `npm run check`
- [x] 8.3 运行增量与 2,000,000 entry release benchmark，保存实际输出并检查全部性能门槛
- [x] 8.4 完成 macOS 与 Windows 手工验收记录，或在不能访问对应平台时明确标记发布阻塞项
- [x] 8.5 运行 `openspec validate complete-runtime-incremental-indexing --strict` 并逐项核对 specs、tasks 和验证证据
