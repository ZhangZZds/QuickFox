## Context

QuickFox 的动作执行集中在 Rust core，前端只展示后端返回的 `secondaryActions` 并调用 `execute_action`。现有 Action 已包含：

- `OpenPath`
- `OpenContainingFolder`
- `CopyText`
- `OpenWithApplication`

新增需求可以复用现有 Action 类型，不需要新增平台 Adapter 或执行命令。

## Decisions

1. **父目录路径由 Rust core 计算。**
   - 对文件结果，父目录是文件所在目录。
   - 对目录结果，父目录是该目录的上级目录。
   - 如果路径没有可表达的父目录，则不返回新增父目录动作。

2. **新增动作复用现有 Action。**
   - “复制所在文件夹路径”使用 `CopyText { text: parent }`。
   - “打开所在文件夹”使用 `OpenPath { path: parent }`。
   - 文件结果现有“打开所在目录”仍保留为 `OpenContainingFolder { path: file }`，用于平台文件管理器定位文件。

3. **前端 label 根据动作和结果类型推导。**
   - 对 `CopyText`，如果 `text` 等于结果 detail/path 的父目录，则显示“复制所在文件夹路径”。
   - 对 `OpenPath`，目录结果自身主/次动作仍显示“打开文件夹”；若 secondary action path 是结果父目录，则显示“打开所在文件夹”。

## Risks

- 文件结果可能同时出现“打开所在目录”和“打开所在文件夹”，两者语义接近。前者用于定位文件，后者打开父目录路径；文案保持清晰以降低困惑。
- Windows 根路径、无父路径等边界不应生成无效父目录动作。
