# Proposal: Polish QuickFox and Prepare 1.0.0 Release

## Summary

完善 QuickFox 1.0.0 前的产品细节：像素狐狸图标、网页搜索回车执行、独立历史模式、更多右键打开方式，以及 macOS/Windows GitHub Release 打包流程。

## Motivation

手工测试继续暴露出几个发布前问题：默认图标不符合产品识别；`bd 1234`
这类网页搜索回车执行不稳定；历史召回和结果列表上下导航冲突；文件右键动作不足；
项目还没有可复用的 release workflow 和 1.0.0 版本标签。

## Scope

- 使用像素风格狐狸作为应用图标和托盘图标。
- 网页搜索前缀输入后，按 Enter 必须打开对应搜索引擎 URL。
- 引入历史查看模式：按住或按下 Shift 进入历史列表，上下键只在历史模式中浏览历史。
- 文件/目录右键菜单增加“开发方式打开”类动作，跨平台使用 Adapter 隔离。
- 将应用版本提升到 1.0.0，补充 GitHub Actions release workflow，支持 tag
  `v1.0.0` 构建 macOS/Windows release 包。

## Out of Scope

- 代码签名、macOS notarization、Windows EV/OV 证书签名。
- Linux release 包。
- 第三方编辑器插件 API。
