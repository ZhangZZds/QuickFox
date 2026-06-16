# QuickFox 1.3.1

## Highlights

- 修复搜索结果主动作激活：鼠标左键点击和 Enter 现在走同一条主动作路径。
- 按结果类型执行默认动作：目录打开文件夹，文件使用系统默认工具打开，应用和网页搜索执行对应主动作。
- 补齐 Esc 分层退出行为：先关闭动作菜单、历史模式、快捷键录制或设置页弹层；搜索栏非空时先清空输入和当前结果；搜索栏为空时才隐藏启动器。
- 修复真实 Tauri 运行时 Esc 隐藏窗口不生效的问题：默认关闭动作改为隐藏当前窗口，并显式声明 `core:window:allow-hide` 权限。
- 同步 OpenSpec 主规格和归档记录，覆盖搜索结果激活与 Esc 退出规则。

## Verification

- `openspec list --json`
- `npm run check`
- `npm run tauri build`

## Release Notes

这是 `v1.3.0` 后的补丁发布，重点修复启动器结果激活和 Esc 键交互的真实桌面行为。macOS 本机包可通过 `npm run tauri build` 生成；Windows 安装包仍建议通过 GitHub Actions release workflow 在推送 `v1.3.1` tag 后生成并验收。
