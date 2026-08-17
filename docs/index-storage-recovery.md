# Windows 索引存储恢复

如果 `%APPDATA%\QuickFox\quickfox.sqlite` 已增长到数 GB，或
`quickfox.sqlite-journal` 持续增长，应先在任务管理器结束所有 QuickFox 进程。

## 可以安全删除的文件

QuickFox 完全退出后，可以删除以下生成文件：

```text
quickfox.sqlite
quickfox.sqlite-journal
quickfox.sqlite-wal
quickfox.sqlite-shm
```

这些文件包含文件名、路径、索引元数据和 QuickFox 使用历史，不包含原始文件正文；
删除不会删除 C/D 盘的用户文件。不要在 QuickFox 或 SQLite 管理工具仍打开数据库时
只删除 journal/WAL 文件。

已有超大数据库不自动执行 `VACUUM`，因为该操作可能需要接近数据库大小的额外磁盘
空间。要立即归还磁盘空间，应同时删除上述数据库文件并重新生成。

## 重建建议

Windows 新配置默认索引当前可用的本地固定磁盘；系统会自动跳过 Windows、ProgramData、
Program Files、恢复/升级目录、回收站、卷元数据、AppData 和虚拟内存文件。应用入口
单独从系统与用户开始菜单建立索引。v1.6.1 完全未修改过的自动
热路径默认会迁移为盘符范围；如果你改过任一索引设置，程序会尊重现有配置。

如果机器规模仍超过 8 GiB baseline 或 5 GiB 剩余空间保护线，可以保留
`config.toml`，把 `[index].include_dirs` 手工缩小为真正需要搜索的具体目录。例如：

```toml
[index]
include_dirs = [
  "C:/Users/<用户名>/Desktop",
  "C:/Users/<用户名>/Documents",
  "D:/Projects",
]
performance_mode = "balanced"
```

新版本会阻止第二个 QuickFox 实例。扫描期间每 2,048 条持续写入同一个
`quickfox.sqlite` 的 staging 表，因此不需要额外索引文件；WAL/SHM 只在 SQLite 需要时
出现，文件大小也不等于已扫描磁盘容量。

Windows 会同时扫描最多 2 个完整盘符，并在每个目录完成后提交恢复断点。异常退出后，
相同配置直接恢复已完成盘的可搜索预览，并从未完成目录继续，不再重扫整盘。每个盘符
使用独立 watcher；一个盘临时离线不会关闭其他盘的增量更新。单个子目录读取失败时，
旧 baseline 中的对应范围会保留并显示为“部分可用”。最终激活后回收失效完整批次，并在
写入可能突破 8 GiB baseline 或 5 GiB 剩余空间保护线时停止。
