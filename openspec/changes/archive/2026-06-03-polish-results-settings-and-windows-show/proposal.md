## Why

QuickFox 1.1.0 的搜索结果和设置页已经可用，但当前展示密度、路径截断、选中反馈和跨平台窗口唤醒仍影响日常使用。维护者在 macOS 截图和 Windows 体验中发现结果无法快速确认命中项、设置页信息不完整，以及托盘“显示 QuickFox”在 Windows 上失效。

## What Changes

- 优化搜索结果行布局：文件名和路径固定对齐，减少路径列留白，默认展示更可辨认的路径摘要。
- 搜索结果默认保留完整文件名，不再因标题列过窄导致命中文件本身不可辨认。
- 路径摘要展示开头路径和结尾若干段路径，悬停时用 tooltip 显示完整路径。
- 强化选中状态视觉反馈，并确保键盘上下移动在 Windows 上也能触发明显的选中变化。
- 修复 Windows 托盘“显示 QuickFox”入口，复用统一窗口显示/聚焦状态路径。
- 设置页索引分区完整展示索引状态、索引快照文件位置和配置文件位置。
- 设置页保存按钮从各内容分区中移到左侧统一操作区。

## Capabilities

### New Capabilities

### Modified Capabilities

- `launcher-shell`: 改进结果列表可读性、路径 tooltip、选中状态和设置页统一保存布局。
- `configuration-and-history`: 设置页需要显示配置文件位置，并让保存动作成为设置页级别操作。
- `search-index`: 设置页需要显示索引快照文件位置和更完整的索引状态。
- `actions-and-platform`: 托盘显示入口必须在 Windows、macOS、Linux 上可靠显示并聚焦主窗口。

## Impact

- 前端：`src/App.tsx`、`src/styles.css`、`src/App.test.tsx`。
- Tauri client contract：新增或扩展设置页可读取的应用路径信息。
- Rust core：暴露配置/索引文件位置，调整托盘显示窗口路径，并补充单元测试。
- 文档/验收：更新 Windows/macOS/Linux 手工验收清单中托盘显示和设置页路径信息检查点。
