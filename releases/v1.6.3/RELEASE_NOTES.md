# QuickFox 1.6.3

这是一个面向 Windows 全盘索引稳定性、恢复能力和可观测性的修复版本。C/D 等多个
盘符会独立推进并逐盘变为可搜索；单个目录不可访问、磁盘短暂离线或应用异常退出，
不再否决已经完成的全盘基线。

## Highlights

- 全盘扫描每 2,048 条持续写入带配置指纹的 SQLite staging，数据库在扫描过程中会
  持续增长；异常退出后可跳过已完成的盘符和目录，不再从头重扫。
- Windows 使用系统 API 只枚举本地固定磁盘，数据盘优先于系统盘；系统噪音目录继续
  排除，`Program Files` 中的应用入口改由开始菜单索引提供。
- C/D 等完整盘符最多并行扫描 2 个，先完成的盘立即可搜索；设置页逐盘展示“等待中、
  扫描中、可搜索、部分可用”和独立统计。
- 每个索引根目录使用独立原生 watcher；单盘离线、注册失败或局部目录读取失败不会
  关闭其他盘的增量更新。
- 待命 watcher 的溢出或临时读取失败不再触发
  `standby capture handoff requires a recovery scan` 并丢弃已完成基线；现在先发布
  可用结果，再在后台校准不确定目录。
- 部分子目录读取失败时保留该范围的 last-known-good 条目，恢复后继续校准，避免
  文件从搜索结果中永久消失。
- 启动时校验 active baseline 的配置指纹和目录 manifest；匹配时直接恢复 watcher，
  不再每次启动都重扫 C/D。
- 名称/路径索引与正文索引共享同一 SQLite 批次，最终紧凑索引直接从 SQLite 流式
  构建，降低大文件树扫描时的内存峰值。

## Upgrade notes

- 升级后无需删除 `quickfox.sqlite` 或重新配置索引目录；未完成的 staging 会自动恢复。
- 已完成的盘符会先保持可搜索，后台继续处理未完成或需要校准的盘符和目录。
- 全盘 baseline 继续受磁盘空间预算保护；达到上限时保留最近可用索引并提示缩小范围。
- 命令执行仍默认关闭；危险命令检查是防护栏，不是权限沙箱。

## Verification

- 318 个前端测试通过。
- 552 个 Rust 测试通过，6 个 release/benchmark 测试按设计忽略。
- Prettier、ESLint、TypeScript/Vite build、rustfmt、Clippy 和 `git diff --check`
  全部通过。
- GitHub Actions 将在 Ubuntu、Windows runner 上执行 CI，并在 macOS、Windows runner
  上构建正式安装包。

## Validation scope

本版本的 Windows 多盘、临时离线、超大 NTFS 文件树和休眠/唤醒行为已覆盖自动化逻辑
测试，但尚未在真实 Windows C/D 盘环境完成发布包手工验收；升级后请按
`docs/windows-manual-qa.md` 记录实际扫描、重启恢复和资源占用结果。
