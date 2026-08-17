# QuickFox 1.6.2

这是一个 Windows 索引配置与默认范围修复版本。配置保存不再等待全盘扫描，后台失败
也不会把已经保存的模式或目录回滚；Windows 新配置默认覆盖当前可用盘符，同时自动
跳过系统噪音目录。

## Highlights

- `fast`、`balanced`、`complete` 切换会先持久化 desired config，再在后台应用索引；
  关闭并重开设置或重启应用后仍保留最后一次保存值。
- 设置页区分保存中、已保存、索引应用中、部分应用和应用失败，不再静默假装成功。
- Windows 首次配置默认包含当前可用的盘符根目录；没有可用盘符时回退用户 profile。
- v1.6.1 完全未修改的自动热路径默认会迁移为盘符范围；任一索引字段被修改时保持
  用户配置。
- 默认跳过 `Windows`、`ProgramData`、`PerfLogs`、恢复/升级目录、回收站、卷元数据、
  `AppData` 和虚拟内存文件；`Users`、Desktop、Documents 普通数据仍可索引。
- 单个盘符或目录不可访问时，其他可用位置继续应用并显示 partial；恢复后可通过
  “重试/校准索引”继续收敛。
- 连续保存新设置会取消旧 revision 的扫描；`fast` 不会因配置仍包含整盘 root 而继续
  扫描整盘。
- Windows 释放内容索引版本时会对短暂的目录占用做有界重试，避免退出或切换索引后
  因文件句柄刚释放而遗留临时目录。

## Upgrade notes

- 升级后无需删除配置或索引数据库。
- Windows 默认全盘补全仍使用 `balanced` 分阶段执行：应用入口和用户热路径先可用，
  盘符范围随后在后台补全。
- 全盘 baseline 继续受 8 GiB 估算上限和至少保留 5 GiB 可用空间的保护；超限时保留
  最近可用索引并提示缩小范围。
- 命令执行仍默认关闭；危险命令检查是防护栏，不是权限沙箱。

## Verification

- `npm run check`
  - 318 个前端测试通过
  - 542 个 Rust 测试通过，6 个 release/benchmark 测试按设计忽略
  - Prettier、ESLint、TypeScript/Vite build、rustfmt、Clippy 全部通过
- `openspec validate fix-index-settings-state-transitions --strict`
- `openspec validate default-windows-full-drive-indexing --strict`
- `git diff --check`
- GitHub Actions 在 Ubuntu 与 Windows runner 上执行格式、lint、前后端测试和构建；
  release workflow 在 macOS 与 Windows runner 上生成安装包。

## Validation scope

真实 Windows C/D 多盘、离线盘、NTFS 大目录资源占用和重启恢复仍需按
`docs/windows-manual-qa.md` 留存发布构建截图与数据；未完成前不得将这些桌面场景标记为
已手工验收。
