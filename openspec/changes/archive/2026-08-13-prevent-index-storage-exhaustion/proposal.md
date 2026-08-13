## Why

Windows 首次运行会默认扫描所有盘符。文件量较大时，完整索引使用单个 SQLite
事务写入，`quickfox.sqlite-journal` 可增长到 GB 级；旧完整批次又不会及时回收，
主数据库会持续累积。QuickFox 同时没有单实例保护，重复启动会进一步放大扫描和
数据库争用，存在写满系统盘的风险。

## What Changes

- 将 Windows 首次默认范围收窄到现有的 Desktop、Documents、Downloads、Projects
  和 workspace 等用户高价值目录；找不到这些目录时回退用户 profile。
- 使用官方 Tauri single-instance 插件阻止第二个后台实例；重复启动改为唤起已有
  实例的主窗口。
- 为完整 baseline 增加 `building/completed` 状态并分块提交；崩溃残留的 building
  批次不会参与恢复，并在重启时清理。
- 新数据库使用 WAL、自动 checkpoint、journal size limit 和 incremental auto-vacuum。
- 激活 baseline 或保存新 checkpoint 后清理失效的完整批次；不再持久化可从
  name/path 推导的 `search_text`。
- 写入 baseline 前估算空间，单批估算超过 8 GiB 或写入后不能保留 5 GiB 空闲空间
  时拒绝持久化并提示缩小索引目录。

## Capabilities

### Modified Capabilities

- `launcher-shell`: 桌面应用只允许一个运行实例，重复启动唤起已有实例。
- `search-index`: Windows 首次默认范围、完整 baseline 事务、保留策略和磁盘安全边界。

## Impact

- Rust/Tauri 启动入口与依赖。
- SQLite schema、baseline 写入、恢复和回收路径。
- Windows 首次配置行为与手工验收文档。
- 与旧版自动生成值完全一致的 Windows 全盘配置会迁移到安全默认范围；改过任一索引
  选项的显式配置保持不变。
- 已膨胀的旧数据库不会自动执行高风险 `VACUUM`；用户需退出后删除并重建一次才能
  立即归还磁盘空间。
