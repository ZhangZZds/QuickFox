# QuickFox 1.1.0

## Highlights

- 后台构建并持久化文件索引，启动时优先加载最近完成的 SQLite 索引快照。
- 文件索引不可用或构建中时显示状态反馈，计算器、网页搜索和命令模式保持可用。
- 优化文件搜索路径，缓存规范化匹配文本并限制候选结果构造。
- 设置页升级为分区控制台，支持索引 include/exclude 规则和网页搜索引擎管理。
- 默认新增 DuckDuckGo 搜索前缀 `ddg`。
- Windows 发布版配置为 GUI 子系统，避免后台运行时弹出额外 console 窗口。

## Local Artifacts

- `QuickFox_1.1.0_aarch64.dmg`
- `QuickFox_1.1.0_aarch64.app.zip`
- `SHA256SUMS.txt`

Windows 安装包由 GitHub Actions release workflow 在 `windows-latest` runner 上构建并上传到 GitHub Release。
