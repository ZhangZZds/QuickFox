# Design: Polish QuickFox and Prepare 1.0.0 Release

## Icon

图标采用仓库内可维护的像素 SVG，并同步生成 PNG 给 Tauri 托盘和 bundle 使用。
图标不依赖外部素材，避免版权和构建可用性问题。

## Web Search Enter

前端执行路径应避免依赖异步结果列表是否已经渲染。对于配置过的网页搜索前缀，
`Enter` 可以从本地配置直接构造 `OpenUrl` action 并记录输入历史。
后端 Provider 继续保留搜索结果，用于列表展示。

## History Mode

默认上下键服务于结果列表。历史召回改为显式模式：

- `Shift` 切换/进入历史模式。
- 历史模式显示最近输入列表。
- 上下键浏览历史项。
- Enter 将历史项放回输入框并退出历史模式。
- Escape 退出历史模式；再次 Escape 才关闭窗口。

这样能避免用户有搜索结果时上下键在历史与结果之间抢焦点。

## Open With Development Tool

Action 增加 `OpenWithApplication`，由 Rust core 集中执行。
第一版提供“用开发工具打开”动作：

- macOS：优先 `code`、`cursor`，再尝试 `open -a Visual Studio Code`。
- Windows：优先 `code.cmd`、`cursor.cmd`、`code.exe`、`cursor.exe`。
- Linux：优先 `code`、`cursor`、`xdg-open` 回退。

Provider 只产出统一 action，不直接执行平台命令。

## Release

版本统一更新到 `1.0.0`。新增 `release.yml`：

- 触发：push tag `v*` 或手动 dispatch。
- matrix：`macos-latest`、`windows-latest`。
- 使用官方 `tauri-apps/tauri-action@v0` 创建/上传 GitHub Release 资产。
- 保留普通 CI 不发布安装包。

本地可验证 `npm run check` 和 `npm run tauri build`；跨平台包由 GitHub-hosted
runner 生成。
