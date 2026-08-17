# Changelog

本文件记录 QuickFox 的用户可见变化。完整安装包和历史版本说明见
[GitHub Releases](https://github.com/ZhangZZds/QuickFox/releases)。

## [1.6.1] - 2026-08-13

### Added

- 新增桌面单实例保护；重复启动会唤起已有 launcher，不再创建第二套托盘、全局
  快捷键监听和后台索引任务。
- 新增完整 baseline 的磁盘预算：单批估算上限 8 GiB，并强制为系统盘保留至少
  5 GiB 可用空间。

### Changed

- Windows 首次默认索引范围改为当前用户实际存在的 Desktop、Documents、Downloads、
  Projects 和 workspace 等高价值目录，不再自动扫描 `C:\`、`D:\` 根目录。
- 与旧版自动生成全盘配置完全一致的配置会安全迁移；任一索引字段已自定义时保持
  原配置。
- 完整 baseline 改为 2,048 条一批的可恢复事务；新数据库使用 WAL、自动 checkpoint、
  64 MiB journal 保留上限和 incremental auto-vacuum。
- 激活新 baseline 后回收旧完整批次；阶段 checkpoint 只保留 active baseline 与最新
  checkpoint；不再持久化可从 name/path 推导的 `search_text` 副本。

### Fixed

- 修复 Windows 大文件树首次索引时 `quickfox.sqlite-journal` 持续增长到 GB 级、旧完整
  批次长期累积并可能写满系统盘的问题。
- 修复未完成 baseline 在异常退出后缺少显式状态和清理路径的问题。

### Verification

- `npm run check`：313 个前端测试通过；529 个 Rust 测试通过，6 个显式 release/
  benchmark 测试按设计忽略；Prettier、ESLint、TypeScript/Vite build、rustfmt 和
  Clippy 全部通过。

### Validation scope

- GitHub Actions 将构建 macOS 与 Windows 安装包。Windows 发布构建上的重复启动、
  SQLite/WAL 峰值与低空间保护仍需真实机器记录；现有 17.5 GB 旧数据库应在 QuickFox
  完全退出后删除并重建，程序不会自动执行可能额外占用大量空间的 `VACUUM`。

## [1.6.0] - 2026-08-11

### Added

- 新增默认开启的运行期自动增量索引：文件创建、修改、删除和重命名通过有界
  watcher 队列、5 秒静默窗口和 10 秒硬上限批量进入搜索结果。
- 新增 targeted scanner、目录 manifest、baseline/overlay/tombstone 分层搜索视图和
  SQLite delta journal；应用异常退出后可重放已经提交的增量。
- 新增运行期索引状态，包括 Watching、Calibrating、Degraded、pending/dirty 数量、
  最近批次摘要和结构化降级原因。
- name/path 增量与正文索引更新解耦，正文读取失败不会回滚已经可搜索的文件名。

### Changed

- 普通增量只更新 overlay，不重建完整 compact baseline；达到 50,000 条或 64 MiB
  安全阈值后才安排后台基线刷新。
- 设置页帮助提示改为窗口级 portal 浮层，并在 hover、键盘聚焦、滚动、窗口缩放及
  窗口边缘场景重新定位。
- canonical 文件系统路径只用于识别 watcher 事件所属根目录；索引和 manifest 保留
  用户配置的稳定根路径身份。

### Fixed

- 修复 Tauri setup 完成后启动索引任务仍可能等待 `RunEvent::Ready`，导致配置目录未被
  刷新且 watcher 没有启动的问题。
- 修复旧数据库的 `index_runtime_state` 与 `index_delta_batches` 不会被
  `CREATE TABLE IF NOT EXISTS` 升级的问题；缺少 `payload_hash` 的旧 journal 现在会在
  保留可搜索 baseline 的前提下安全迁移。
- 修复 macOS watcher 重扫信号触发不必要全量刷新、排除目录事件挤占有界队列，以及
  `/var` 与 `/private/var` 路径别名造成事件丢失的问题。
- 修复设置页提示框被滚动容器裁切、遮挡和移出可视区域的问题。
- 补强大索引候选召回、Unicode 路径大小写匹配、内存计数和持久化恢复错误传播。

### Verification

- `npm run check`：313 个前端测试通过；520 个 Rust 测试通过，6 个显式 release/
  benchmark 测试按设计忽略；Prettier、ESLint、TypeScript/Vite build、rustfmt 和
  Clippy 全部通过。
- macOS 真实用户数据库验证：新建空文件提交为 generation 5 `upsert`，删除提交为
  generation 6 `remove`，active baseline 未变化。
- 维护者确认设置页 tooltip 与新增文件搜索验收通过，并授权发布 v1.6.0。

### Validation scope

- GitHub Actions 会构建 macOS 与 Windows 安装包。Windows NTFS 多盘、休眠/唤醒和
  断盘恢复的完整结构化手工记录本次未补录；维护者已明确接受该发布验证边界。

[1.6.1]: https://github.com/ZhangZZds/QuickFox/compare/v1.6.0...v1.6.1
[1.6.0]: https://github.com/ZhangZZds/QuickFox/compare/v1.5.0...v1.6.0
