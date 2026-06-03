## 1. OpenSpec 与设计确认

- [x] 1.1 创建 proposal/design/spec/tasks 并通过 `openspec validate polish-results-settings-and-windows-show --strict`
- [x] 1.2 获得维护者对结果列表、设置页和 Windows 显示修复设计的确认

## 2. 结果列表展示

- [x] 2.1 先为路径摘要、完整路径 tooltip、标题可辨认和选中态写前端测试
- [x] 2.2 实现路径摘要 helper 和结果行语义属性
- [x] 2.3 调整结果列表 CSS，使标题/路径对齐、减少留白、选中态更明显

## 3. 设置页信息架构

- [x] 3.1 先为统一保存入口、配置文件路径和索引快照路径写前端/Tauri contract 测试
- [x] 3.2 Rust core 暴露应用路径信息，包含配置文件和索引快照路径
- [x] 3.3 设置页展示完整索引状态和路径信息，并将保存按钮移入左侧统一操作区

## 4. Windows 托盘显示

- [x] 4.1 先为托盘显示同步窗口状态写 Rust 单元测试
- [x] 4.2 修复托盘“显示 QuickFox”复用统一窗口显示/状态逻辑
- [x] 4.3 更新 Windows/macOS/Linux 手工验收清单中的托盘显示检查点

## 5. 验证

- [x] 5.1 运行 `npm run test`
- [x] 5.2 运行 `npm run rust:test`
- [x] 5.3 运行 `npm run check`
- [x] 5.4 运行 `openspec validate polish-results-settings-and-windows-show --strict`

## 6. 结果类型与动作菜单

- [x] 6.1 先为应用/文件/目录动作菜单差异写 Rust 和前端测试
- [x] 6.2 实现应用/文件/目录的后端 secondary actions 区分
- [x] 6.3 搜索结果列表显示应用/文件/目录类型标识

## 7. 设置窗口与 Shift+Shift 唤醒

- [x] 7.1 为设置内容滚动容器和热键状态展示补前端测试
- [x] 7.2 固定设置面板尺寸，右侧内容区域独立滚动，并允许 Tauri 窗口调整大小
- [x] 7.3 后端暴露 Shift+Shift 全局监听状态，权限失败时提供可操作提示
- [x] 7.4 调整设置内容区填满可用空间、切换分区回到顶部，并提供 macOS 输入监控设置入口
