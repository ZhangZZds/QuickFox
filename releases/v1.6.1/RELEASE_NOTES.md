# QuickFox 1.6.1

这是一个 Windows 索引存储安全补丁，重点修复大文件树首次运行时 SQLite 数据库与
journal 可能异常增长、重复启动会并行扫描的问题。

## Highlights

- QuickFox 现在只允许运行一个实例；重复启动会唤起已有 launcher。
- Windows 新安装不再默认扫描整个 C/D 盘，只索引用户常用目录；完全未修改过的旧版
  全盘默认配置会自动迁移，自定义配置保持不变。
- 完整 baseline 改为可恢复的分块事务，未完成批次不会进入搜索并会在重启时清理。
- 激活新 baseline 后自动回收旧完整批次和过期 checkpoint。
- 新数据库使用 WAL、自动 checkpoint、64 MiB journal 保留上限和增量页回收。
- baseline 估算超过 8 GiB，或不能保留至少 5 GiB 可用空间时，索引会停止持久化并
  保留最近可用结果。

## Existing oversized databases

如果 `%APPDATA%\QuickFox\quickfox.sqlite` 已经增长到数 GB，请先在任务管理器结束所有
QuickFox 进程，再删除 `quickfox.sqlite` 以及同名的 `-journal`、`-wal`、`-shm` 文件后
重建索引。这些是索引数据库，不是原始文件。程序不会自动对超大旧库执行 `VACUUM`，
以免在空间不足时产生接近数据库大小的额外临时占用。

## Verification

- `npm run check`
  - 313 个前端测试通过
  - 529 个 Rust 测试通过，6 个 release/benchmark 测试按设计忽略
  - Prettier、ESLint、TypeScript/Vite build、rustfmt、Clippy 全部通过

## Validation scope

GitHub Actions 会构建 macOS 与 Windows 安装包。Windows 发布构建上的重复启动、
SQLite/WAL 峰值和低空间保护仍需在真实 Windows 机器上记录。
