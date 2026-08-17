# QuickFox 1.2.1

## Highlights

- 修复设置窗口返回搜索后仍停留在大尺寸设置窗口外壳中的问题。
- 修复索引异常状态下点击“打开设置”偶发无法唤起独立设置窗口的问题。
- 设置窗口现在可通过 Tauri window label 兜底识别设置视图，避免 query 参数丢失时误渲染启动器。
- 启动器和设置页的窗口命令分离：启动器保持紧凑浮层，设置页保持系统窗口控件和可调整大小能力。
- 整理启动器首用体验、索引状态反馈和视觉验收文档。

## Verification

- `npm run check`

## Release Notes

这是 `v1.2.0` 后的补丁发布，重点修复手工验收发现的窗口路由和设置入口回归。macOS 和 Windows 安装包由 GitHub Actions release workflow 在 tag 推送后构建并上传到 GitHub Release。
