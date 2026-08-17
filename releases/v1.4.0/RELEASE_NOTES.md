# QuickFox 1.4.0

## Highlights

- 优化首次索引体验：`fast` / `balanced` / `complete` 模式现在会影响扫描计划、阶段优先级和默认范围，让启动后先快速可用，再在后台补齐完整索引。
- 降低索引阶段卡顿：索引快照保存增加节流，内容索引延后为低优先级任务，避免阶段边界频繁重建重量级数据。
- 优化已建立索引后的搜索输入体验：前端搜索增加节流和过期结果保护，Rust Provider 查询避免克隆完整索引，连续打字更稳。
- 设置页和启动器补充索引状态反馈：可区分快速可用、后台补全和内容索引阶段，并说明性能模式取舍。
- 搜索结果右键菜单新增“复制所在文件夹路径”和“打开所在文件夹”，文件与目录结果都支持定位父目录。
- 修复 Windows 全局唤醒键状态残留导致的误触发风险，并在快捷键录制时提示系统保留或高风险组合键。

## Verification

- `npm run check`
- GitHub Actions Release workflow on tag `v1.4.0`

## Release Notes

这是 `v1.3.1` 后的功能发布，重点改善首次启动和大目录索引时的体感，并补齐搜索结果定位文件夹的右键动作。macOS 与 Windows 安装包由 GitHub Actions release workflow 在推送 `v1.4.0` tag 后生成并上传到 GitHub Release。
