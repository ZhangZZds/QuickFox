## Why

QuickFox 1.6.x 将索引设置持久化错误地绑定到新索引完整校准和原子切换：Windows 大目录、离线盘符或单个读取失败会让保存长时间阻塞或整体失败，而设置页没有反馈，用户再次打开时仍看到旧模式。设置保存必须表达用户的期望配置，索引应用则应作为可观察、可恢复、可被新 revision 取代的后台过程。

## What Changes

- 将“期望配置已保存”与“索引 revision 已应用”拆成两个明确状态；索引语义设置通过校验后先持久化并快速返回。
- 保存索引语义变化后保留最近可用搜索视图，在后台按新 revision 构建和切换索引；失败不回滚已经保存的用户选择。
- 后台应用支持 revision supersede 和扫描中取消，避免连续保存时旧大目录扫描继续占用磁盘 I/O。
- 让 `fast`、`balanced`、`complete` 同时约束 baseline 扫描、校准和 watcher 范围，避免 `fast` 被配置大目录绕过。
- 将单 root/目录项失败降级为结构化 root 失败；其他可用范围继续完成，只有没有任何可用范围时才整体失败。
- 设置页显示未保存、保存中、配置已保存/索引应用中、部分可用、失败和已应用状态，保存失败必须可见；删除“保存后还需手动刷新”的矛盾说明。
- 为 Windows 盘符根目录增加风险说明和真机验收，覆盖模式切换、大目录、不可访问目录、连续保存和重启恢复。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `configuration-and-history`: 明确索引设置先持久化、后台应用、失败不回滚用户期望配置，以及性能模式对全部索引生命周期范围的一致语义。
- `search-index`: 增加可取代的配置 revision、可取消后台扫描、部分 root 失败和旧搜索视图保留要求。
- `launcher-shell`: 增加设置草稿、保存和索引应用状态反馈，统一保存与刷新交互语义。
- `project-quality`: 增加 Windows 设置状态流与大目录 I/O 的真实桌面回归验收。

## Impact

- Rust core：配置事务、索引 refresh/revision 生命周期、扫描取消、watcher/calibration root 选择、索引状态结构和持久化恢复。
- Tauri contract：`save_config` 返回结构化保存/应用结果，索引状态补充目标 revision 与应用状态。
- React 设置页：dirty/save/apply/error 状态、按钮行为、帮助文案和索引状态展示。
- 测试与文档：Rust/前端回归测试、Windows 手工 QA、架构与故障排查说明。
- 不改变 Provider/Action 安全边界，不新增第三方插件 API，不自动扩大用户配置的索引范围。
