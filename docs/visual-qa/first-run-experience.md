# QuickFox 首次使用体验视觉验收

日期：2026-06-07

对应功能：首次使用体验

## 覆盖场景

- 启动器空输入状态。
- 文件索引未建立、构建中、失败和无结果状态。
- 搜索有结果状态。
- 设置页入口和设置窗口布局。
- 普通 Vite 浏览器页面中的 Tauri event listen 降级。

## 自动化验收

- `src/App.test.tsx` 覆盖空输入不显示状态区域、中文 placeholder、索引未建立/构建中/失败/无结果/有结果状态、恢复动作、非文件 Provider 不被索引状态覆盖、索引 ready 事件触发当前查询刷新。
- `src/tauriClient.test.ts` 覆盖 `quickfox://index-status` 监听，以及 Tauri event bridge 缺失时返回 noop unlisten。
- Rust 单元测试覆盖 launcher/settings 窗口形态分离、托盘 show/settings 路由分离、索引完成/失败后生成前端事件 payload。

## 浏览器布局观察

在 `http://127.0.0.1:1420/` 中观察到：

- 空输入状态只显示紧凑搜索框；`.launcher-panel` 高度约 60px，`.search-input` 高度 58px。
- 空输入状态没有结果列表留白和启动器状态区域。
- `http://127.0.0.1:1420/?view=settings` 能直接进入设置视图；`.settings-panel` 约 920x640，右侧 `.settings-content` 独立占据内容区。

浏览器插件截图和输入模拟在本机环境中受限，截图命令超时，输入模拟受虚拟剪贴板限制；因此本轮用 DOM 布局检查 + 自动化测试作为等价视觉验收。真实 Tauri 透明窗口、系统标题栏和桌面背景污染仍需按 macOS/Windows/Linux 手工验收清单复验。

## 待人工确认

- 真实桌面里启动器是否没有系统标题栏和设置页导航。
- 真实桌面里设置页是否打开独立设置窗口，并保留系统关闭、缩放和调整大小入口。
- 托盘“显示 QuickFox”只显示启动器；托盘“设置”只显示设置页。
- 后台索引完成时，已输入的普通查询能自动刷新结果。
