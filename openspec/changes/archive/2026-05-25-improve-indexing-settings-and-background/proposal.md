## Why

QuickFox 在 Windows 上面对数百 GB 文件树时，启动和搜索会被同步全量索引拖慢；用户还缺少清晰的索引状态反馈和图形化 Provider 配置入口。现在需要把文件索引变成后台、持久、可观察的能力，同时让设置页可以管理 DuckDuckGo 等网页搜索引擎，并保持 macOS/Linux 行为不退化。

## What Changes

- 将启动期索引从阻塞式全量扫描改为后台索引流程；启动器、托盘、计算器、网页搜索、命令模式在索引未完成时仍可用。
- 将文件索引持久化到本地 SQLite，并在启动时加载上次可用索引；后台刷新完成后原子替换可搜索快照。
- 优化文件搜索的内存结构和匹配路径，避免每次查询对全部记录重复拼接和大小写转换。
- 为索引状态提供 Rust 命令和前端提示：未建立、建立中、可用、失败或使用旧索引。
- 在 Windows 发布构建中避免后台常驻时弹出 cmd/console 窗口；继续支持 Shift+Shift 唤醒，macOS/Linux 保持托盘常驻和现有唤醒能力。
- 重做设置页为分区式控制台体验，并为新增/编辑网页搜索引擎提供轻量向导式弹层；默认或示例支持 DuckDuckGo。
- 更新跨平台 QA 文档，明确 Windows 大文件树、无控制台窗口、macOS 权限和 Linux fallback 的验收点。

## Capabilities

### New Capabilities

- None.

### Modified Capabilities

- `search-index`: 后台索引、持久化索引快照、索引状态、搜索性能约束和失败恢复。
- `launcher-shell`: 启动非阻塞、索引不可用提示、设置页分区体验和 Shift+Shift 常驻唤醒约束。
- `configuration-and-history`: 图形化管理索引规则和网页搜索引擎配置，包括 DuckDuckGo。
- `query-providers`: 网页搜索 Provider 使用可配置引擎，并覆盖新增 DuckDuckGo 的前缀/URL 模板行为。
- `actions-and-platform`: Windows 无控制台窗口常驻，以及 macOS/Linux 保持平台启动和唤醒行为。
- `project-quality`: 增加跨平台性能和桌面行为验收文档要求。

## Impact

- Rust core: `index`, `storage`, `config`, `providers`, `search`, `platform`, Tauri app state 和命令接口。
- Frontend: `App.tsx`, `tauriClient.ts`, 设置页布局、索引状态提示、网页搜索引擎管理交互和测试。
- Packaging/platform: Windows 子系统配置、Tauri 窗口启动策略、托盘/热键生命周期。
- Storage: SQLite schema 需要新增或迁移索引条目表、索引状态表字段和查询辅助字段。
- Tests/docs: Rust 单元测试、前端组件测试、Tauri client contract 测试、OpenSpec delta、Windows/macOS/Linux 手工 QA 文档。
