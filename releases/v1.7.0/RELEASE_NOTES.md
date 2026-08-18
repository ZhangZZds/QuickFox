# QuickFox 1.7.0

QuickFox 1.7.0 聚焦索引可靠性、快速启动和大规模文件树恢复。完整刷新现在具有明确、可
恢复的代际协议；已完成盘符在后台扫描和最终构建期间持续可搜索，普通启动也不会因为
已有兼容索引而重新执行全盘扫描。

## Highlights

- 索引代际使用 `building -> prepared -> active -> obsolete`。扫描中断从目录 checkpoint
  继续，prepared 中断只继续最终构建和激活；Active 切换与 manifest/runtime generation
  在同一 SQLite 事务提交。
- 搜索视图在完整刷新期间合并旧 Active、已完成根的 Preview 和增量层。finalizing 或
  activation 失败不会让已经发布的 D 盘等结果消失。
- 托盘、快捷键和最小 Runtime 先创建；SQLite Active 恢复、校准和存储维护在 setup 后
  异步进行。第二实例无需加载完整索引即可唤醒已有进程。
- Windows 固定 NTFS 卷新增无服务 Win32 批量枚举 Adapter，并支持目录断点、流式批次、
  进度和取消；非 NTFS、网络/移动盘、能力失败或语义不兼容时回退 Generic Scanner。
- watcher 根会合并嵌套覆盖关系，每个磁盘独立降级。活跃全量刷新会吸收对应 dirty-root
  事件，避免扫描刚结束又开始第二轮全盘刷新。
- SQLite 新库启用 incremental auto-vacuum 和有界 WAL；后台清理 obsolete/orphan
  generation，并为旧 `auto_vacuum=NONE` 数据库安排非阻塞迁移。
- 设置页展示明确的索引阶段、刷新原因和存储诊断，且最大化后使用完整窗口区域；后台
  恢复不会把仍然可用的搜索描述为安装失败。

## Upgrade notes

- 无需手工删除现有 `quickfox.sqlite`。兼容 Active 会直接恢复；未完成的 building 或
  prepared 会从持久化状态继续。
- 旧数据库的空间回收迁移在后台执行，不阻塞托盘和主窗口；SQLite 正忙或空间不足时会
  保留 Active 并延后维护。
- 当前版本只探测 USN Journal 能力，不读取原始 MFT、不请求 UAC，也不安装高权限服务。
- 命令执行仍默认关闭；危险命令检查是防护栏，不是权限沙箱。

## Verification

- `npm run check` 通过。
- 339 个前端测试通过。
- 572 个默认 Rust 测试通过；6 个 release/benchmark 测试按设计默认忽略。
- 两项 200 万条目 release 测试额外执行通过：估算常驻内存约 440.7 MiB，受测常用查询
  最慢约 51.6ms。
- Prettier、ESLint、TypeScript/Vite build、rustfmt、Clippy `-D warnings` 和
  `git diff --check` 全部通过。

## Validation scope

Windows 标准用户环境下的真实 200 万 NTFS 文件完整扫描、托盘/第二实例 P95、三轮刷新
数据库体积，以及 building、prepared、finalizing、activation 阶段强杀恢复仍需在发布
包上补录。当前 macOS 合成基线不能证明“NTFS 200 万文件 90 秒目标已达成”；实机采样
方法见 `docs/windows-ntfs-index-source.md`。
