# Runtime Indexing Recovery Fence 设计

## 目标

在配置 revision 切换、full-refresh handoff、应用崩溃和启动恢复期间，保证只有与当前索引语义兼容的 journal delta 能进入新 baseline；同时避免重复全量刷新，并让所有后台线程失败走同一可测试恢复路径。

## 核心不变量

1. 配置切换先为新 revision 建立 capture watcher，再 durable handoff 并停止旧 service。旧 service 完成 handoff 后读取的最高 generation 形成 compatibility fence；新 full scan 只吸收严格大于该 fence、且由新 revision capture 产生的 tail。
2. 新 revision watcher 在旧 service 停止前已生效；旧 service join 后，successor runtime worker 在 baseline activation/content 构建期间持续写 journal。successor 不直接发布 UI 状态，baseline 完成前 runtime 保持 Preparing/Refreshing，不得宣称 Ready。
3. Baseline 安装 generation 是其实际 materialize 的 tail 最高 generation。若安装期间又产生新 delta，安装操作以 journal 为权威原子替换 baseline，再按 generation 重放更新的 tail。
4. Delta safety threshold 只在没有 refresh in flight 时发起一次 full refresh；阈值导致的后续 delta 不得将当前 refresh 标记 pending。恢复性 fallback 仍可 latch same-revision rerun。
5. 启动恢复在 manifest calibration 完成前不进入 Watching。缺失 configured root、损坏或不完整 manifest 保持 Degraded，并安排可恢复 refresh。
6. watcher、storage、rules、worker spawn 和 handoff failure 使用同一结构化 failure outcome；生产 thread spawn 与测试注入走相同 seam。

## 模块边界

新增 `core/index_refresh_orchestrator.rs`，承载纯状态与决策：revision fence、refresh request latch、authoritative generation 安装计划、startup calibration gate 和可注入 worker spawner。`lib.rs` 负责 Tauri main-thread dispatch、构造 watcher/scanner/storage，并执行这些 core 决策。

`LayeredSearchIndex` 提供受约束的 baseline install + journal replay API：调用方必须提供从 baseline generation 连续覆盖到最终 generation 的 committed deltas；缺口时拒绝替换，保留旧 view。

## 测试策略

- 每个缺陷先用生产 seam 写失败测试并确认 RED。
- revision 测试同时运行旧 service、新 capture、generation fence 与新配置 rules，证明旧 exclude/root delta 不进入新 baseline。
- crash 测试在 successor event committed 后、baseline activation 前直接重开 SQLite recovery。
- startup 测试从真实 recovery snapshot/manifest 构建 runtime，覆盖 missing root 与离线变化 calibration。
- spawn failure 使用同一个 `RefreshWorkerSpawner` seam 注入失败，并调用生产 failure funnel，断言 handle/standby/status/retry。

## 非目标

不修改 OpenSpec tasks，不增加第三方插件接口，不改变 Provider/Action 安全边界，也不在本轮实现最终后台 compaction。
