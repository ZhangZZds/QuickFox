## Context

QuickFox 当前把影响索引语义的设置保存实现为同步 revision candidate：先注册 watcher、完整扫描所有 candidate roots、构建搜索和内容索引、写入 rollback/candidate/final baseline，最后才持久化 `config.toml`。该方案试图保证 watcher、manifest、SQLite baseline 和内存视图原子切换，却把文件系统可用性变成了配置持久化的前置条件。

结果是 Windows 大目录保存长时间阻塞；任一读取失败会拒绝整个配置；连续保存不能在 root 内及时取消旧扫描；设置页没有反馈；`refresh_capture_roots` 又无条件加入 configured roots，使 `fast` 在配置切换和 watcher/calibration 路径中仍触达大目录。长期规格要求“保存后后台刷新”“单目录项失败继续”和“fast 不补扫配置大根目录”，实现与之不一致。

本变更必须保留最近可用索引、增量 journal 的 generation 顺序和 Tauri/Rust 架构边界。Windows 是主要问题平台，但状态机与取消语义必须由跨平台 core 测试覆盖。

## Goals / Non-Goals

**Goals:**

- 配置通过纯配置校验后先持久化并快速返回，不等待目录遍历、内容索引或 baseline 写入。
- 将当前配置作为 desired revision；旧索引继续搜索，后台构建并原子发布 matching revision。
- 后台失败不回滚 desired config；状态提供失败原因、旧视图可用性和重试入口。
- 新 revision 使旧扫描尽快停止，至少在每个遍历 entry/root 边界检查取消。
- 性能模式决定 baseline、watcher 和 calibration 的同一 active-root 集合。
- 单个不可用 root 不阻止其他 root 完成；不可用 root 进入 dirty/failed 状态并允许后续恢复。
- 设置页明确区分草稿、持久化与后台应用状态。

**Non-Goals:**

- 不在本变更引入第三方索引服务、插件 API 或全新文件选择器依赖。
- 不保证整盘 baseline 在固定时间完成，也不把危险命令执行能力与索引设置混合。
- 不自动把用户选择的 `D:\` 改写为若干子目录；只提供风险反馈和建议。
- 不在本变更归档 OpenSpec change 或发布安装包。

## Decisions

### 1. desired config 与 applied index revision 分离

`config.toml` 和 `QuickFoxRuntime.config` 表示用户已经保存的 desired config。`IndexRefreshControl.config_revision` 表示 desired revision；新增 applied revision/apply state 只描述当前搜索 baseline 是否已经匹配 desired revision。

语义配置保存事务仅执行：并发校验、写入配置文件、递增 revision、标记 `applying`、失效旧服务身份并安排后台 refresh。最近可用 `LayeredSearchIndex` 在新 baseline 成功前不替换。后台失败设置 `failed`，但不恢复旧配置文件。

备选是保留现有 candidate 原子事务并只把命令放到异步线程。该方案仍会让“保存成功”依赖全盘扫描，并在重启后继续显示旧值，因此拒绝。

### 2. 配置保存成功与后台调度失败分别报告

`save_config` 在配置已持久化后即视为保存成功。若 worker/watcher 无法启动，命令仍返回保存成功，同时通过结构化 index status 报告 apply failure。只有配置校验、并发写冲突或 `config.toml` 写入失败才使保存命令失败。

设置页维护本地 dirty/save 状态，并订阅 index status 展示 `applying/applied/failed`。关闭和重开设置页总是从已持久化 desired config 恢复选择。

### 3. 复用现有后台 baseline 发布路径

语义保存不再调用同步 `prepare_config_revision_candidate`。它更新 desired revision 后调用现有 `start_background_index_refresh`；worker 仍先注册 standby capture、分阶段扫描、持久化新 baseline，再在 identity/fingerprint 匹配时发布。

旧 active refresh 保留 identity，但 desired revision 变化会使其 superseded。它退出后由 pending refresh 启动最新 revision，避免同时发布两个 revision。

### 4. scanner 提供协作式取消

`IndexScanner`/`IgnoreScanner` 增加 cancellable scan 入口。后台 worker 使用 revision identity closure，在每个 root 和每个 walker entry 前检查；被取代时丢弃 partial report并启动最新 pending revision。

取消是协作式的，不能中断正在进行的单次操作系统 metadata 调用，但不会等待整个 D 盘 plan 结束。普通非后台调用继续使用永不取消的兼容入口。

### 5. active roots 只来自当前模式扫描计划

新增单一 `active_index_roots(config)` 边界，由 `build_scan_plans` 的实际 stage roots 去重生成，供 baseline standby watcher、runtime watcher 和 calibration 共用。

- `fast`：应用入口和实际存在的用户热路径；即使热路径也出现在 configured roots 中也不能被去重掉。
- `balanced`：fast roots 加用户配置 roots。
- `complete`：balanced roots 加剩余可用盘符。

不存在或不可读 root 不进入 watcher 注册，但保留在扫描报告/dirty 状态；其他可用 root 继续。该选择避免 notify 因一个离线盘拒绝全部 roots。

### 6. 部分 root 失败保留可用结果

scanner 继续记录 per-root failure。只要至少一个 stage/root 完成，后台 refresh 可以发布可用条目并把状态标记为 degraded/partial；失败 root 保持 dirty 并由恢复机制重试。没有任何可用 root 或持久化/一致性失败才使本 revision 整体 apply failure。

旧 baseline 在任何失败路径继续可用。成功的 partial baseline 不应错误显示为完整完成。

### 7. “刷新索引”改为恢复动作

保存索引设置自动安排后台应用。手动刷新只用于用户显式重试失败/脏 root 或请求校准，不再是设置生效的第二个必需按钮。帮助文案必须与此一致。

## Risks / Trade-offs

- [配置已保存但索引暂时仍是旧范围] → UI 同时展示 desired config 和 apply state；旧结果继续可用并标记“后台应用中”。
- [进程在保存后、refresh 启动前退出] → 重启始终按 `config.toml` 安排 startup refresh，旧持久化 baseline 仅作为临时可用视图。
- [取消检查过密影响遍历性能] → 使用轻量 revision/atomic identity 检查；性能回归覆盖大索引扫描。
- [部分 root baseline 让用户误以为完整] → `failures/dirtyRoots/applyState` 保持 degraded，设置页列出失败摘要。
- [移除同步 candidate 会减少强原子 handoff 覆盖] → 保留 baseline 发布时 identity/fingerprint fence 和 standby capture；只有配置持久化从索引切换事务中分离。
- [Windows 盘符 watcher 注册仍可能昂贵] → `fast` 不注册配置盘符；balanced/complete 显示风险并通过真机 QA 验证。

## Migration Plan

1. 增加状态与失败测试，证明当前保存会等待扫描、失败回滚模式、fast roots 泄漏和旧任务不可及时取消。
2. 引入 desired/applied 状态和轻量语义配置提交，保留旧索引并后台刷新。
3. 接入 cancellable scanner 与统一 active-root 选择，删除生产同步 candidate 路径及其只服务旧语义的死代码。
4. 更新前端 contract、状态反馈和帮助文案。
5. 运行完整检查与 Windows QA 清单；若需回滚代码，旧数据库 schema 无需降级，`config.toml` 仍保持兼容。

## Open Questions

无。整盘目录的预估耗时/存储 UI 可在后续诊断与压缩变更中增强，本变更先提供明确风险提示和非阻塞状态流。
