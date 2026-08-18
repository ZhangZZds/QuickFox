# Changelog

本文件记录 QuickFox 的用户可见变化。完整安装包和历史版本说明见
[GitHub Releases](https://github.com/ZhangZZds/QuickFox/releases)。

## Unreleased

## [1.7.0] - 2026-08-18

### Added

- 新增 `building -> prepared -> active -> obsolete` 索引代际协议、逐目录恢复断点和原子
  激活事务；相同配置的 building/prepared 在重启后继续原阶段，不再无条件重扫磁盘。
- 新增统一 `IndexSource` 边界。Windows 固定 NTFS 卷使用无服务 Win32 批量枚举，能力
  或语义不匹配时自动回退 Generic Scanner；USN 只做能力探测，不提权或安装服务。
- 索引状态新增七个明确阶段、八类刷新原因，以及 active/pending generation、数据库、
  WAL、freelist、auto-vacuum 和最近 GC 等存储诊断。

### Changed

- 启动同步阶段只创建托盘、快捷键和最小 Runtime；SQLite Active 恢复、校准、GC 和旧库
  vacuum 迁移移到 setup 后的后台任务，第二实例不再加载完整索引。
- 全量刷新搜索视图改为 Active + Root Preview + Incremental Overlay。已完成根在扫描和
  finalizing 期间持续可搜索，只有新 Active 成功安装后才释放 Preview。
- watcher 根先做祖先覆盖压缩，并按根隔离失败；活跃全量刷新会吸收同 revision 的
  dirty-root 事件，避免紧接着排队第二轮全盘刷新。
- 新 SQLite 使用 incremental auto-vacuum 和有界 WAL；激活后异步回收 obsolete/orphan
  generation，旧 `auto_vacuum=NONE` 数据库通过后台维护迁移。
- 设置页移除 1440 × 960 内容上限，使用完整窗口可视区域；后台恢复或维护降级不会用
  阻塞性错误覆盖仍然可用的搜索结果。

### Fixed

- 修复 finalizing 前释放 Root Preview，导致已经完成的盘符结果暂时消失的问题。
- 修复 prepared/building 异常退出后创建新 generation 并从头扫描的问题。
- 修复嵌套热点目录重复注册 watcher、单根异常影响其他磁盘以及 dirty 事件触发连续全盘
  刷新的问题。
- 修复 generation 配置指纹和根路径可能进入前端存储诊断序列化的问题。

### Verification

- `npm run check` 通过：339 个前端测试、572 个默认 Rust 测试通过；6 个显式
  release/benchmark 测试按设计默认忽略。
- 额外以 release 模式运行两项 200 万条目测试并通过：常驻内存估算约 440.7 MiB，受测
  常用查询最慢约 51.6ms。
- Prettier、ESLint、TypeScript/Vite build、rustfmt、Clippy `-D warnings` 和
  `git diff --check` 全部通过。

### Validation scope

- Windows 标准用户环境下约 200 万 NTFS 文件的 90 秒完整扫描目标、托盘与第二实例
  P95、真实 SQLite 三轮刷新体积及四阶段强杀恢复仍需发布包实机记录；本版本不把 macOS
  合成基线等同于 Windows 验收结论。

## [1.6.3] - 2026-08-17

### Fixed

- 修复 Windows 全盘刷新结束时，待命 watcher 的溢出、临时读取失败或不可访问子目录
  会触发 `standby capture handoff requires a recovery scan`，继而丢弃已经完成的 C/D
  盘基线并反复重扫的问题；现在先发布可用基线，再在后台校准不确定目录。
- 局部目录访问失败不再否决整盘索引或触发完整刷新循环；可访问文件保持可搜索，设置
  页显示为可继续恢复的降级状态。
- C/D 等多个配置根目录改为逐盘扫描计划，并在开始扫描盘符时立即更新当前阶段和
  root，避免界面长时间停留在上一个 `user-hot-paths`/Downloads 阶段。
- 全盘扫描改为每 2,048 条持续写入带配置指纹的 SQLite staging；扫描中数据库会持续
  增长，异常退出后可跳过已完成的 root，不再把数百万条结果同时保留多份内存副本。
- 部分子目录读取失败时保留该范围的 last-known-good 条目，并使目录 manifest 强制
  重新校准；失败范围不会在一次不完整扫描后永久消失。
- Windows 默认盘符通过系统 API 只枚举本地固定磁盘，数据盘优先于系统盘补全；
  `Program Files` 不再参与通用全盘递归，应用入口改由系统和用户开始菜单提供。
- 启动时会校验 active baseline 的配置指纹和目录 manifest；两者匹配时直接恢复增量
  watcher，不再每次启动都重扫 C/D。
- Windows C/D 等完整盘符最多并行扫描 2 个，并按目录持久化恢复断点；先完成的盘立即
  可搜索，异常退出后从未完成目录继续。
- 每个索引根目录使用独立原生 watcher；单盘离线或注册失败不再关闭其他盘的增量更新。
- 设置页新增逐盘“等待中 / 扫描中 / 可搜索 / 部分可用”状态与独立统计，明确区分现有
  搜索可用和后台恢复。

### Verification

- 318 个前端测试与 552 个 Rust 测试通过，6 个显式 release/benchmark 测试按设计
  忽略；Prettier、ESLint、TypeScript/Vite build、rustfmt 和 Clippy 全部通过。

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

[1.6.3]: https://github.com/ZhangZZds/QuickFox/compare/v1.6.2...v1.6.3
[1.7.0]: https://github.com/ZhangZZds/QuickFox/compare/v1.6.3...v1.7.0
[1.6.1]: https://github.com/ZhangZZds/QuickFox/compare/v1.6.0...v1.6.1
[1.6.0]: https://github.com/ZhangZZds/QuickFox/compare/v1.5.0...v1.6.0
