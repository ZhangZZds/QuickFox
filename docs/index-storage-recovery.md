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

Windows 新配置默认索引当前可用盘符；系统会自动跳过 Windows、ProgramData、恢复/
升级目录、回收站、卷元数据、AppData 和虚拟内存文件。v1.6.1 完全未修改过的自动
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

新版本会阻止第二个 QuickFox 实例，使用可恢复的分块 baseline 写入，回收失效完整
批次，并在写入可能突破 8 GiB baseline 或 5 GiB 剩余空间保护线时停止。
