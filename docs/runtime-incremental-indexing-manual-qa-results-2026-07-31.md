# 运行期增量索引验收记录（2026-07-31）

变更：`complete-runtime-incremental-indexing`

记录日期：2026-07-31（Asia/Shanghai）

结论：release synthetic benchmark 已通过；macOS 真实桌面与 Windows 真实桌面验收均未执行，当前仍是发布阻塞。不得用自动化测试或 synthetic benchmark 替代人类 GUI、真实 watcher、NTFS、休眠/唤醒和设备断开验收。

## 环境与证据边界

- 当前 Codex 执行环境不能进行可靠的人类 GUI 交互，也不能代表维护者观察窗口焦点、输入卡顿、托盘、状态截图或系统睡眠/设备断开。
- release benchmark 在 macOS arm64 开发机运行，对应文档提交前实现 HEAD `85a6818`；fixture 是进程内 synthetic 数据，不读取真实 C:/D 文件树。
- 当前阶段性自动化汇总可引用为前端 104 个、Rust 482 个测试，但这不是本记录重新执行得到的最终仓库数字。主代理完成最终 verification 后必须填写下方待填段落；若数字变化，以最终命令输出为准。
- 本记录不包含个人路径、用户名、卷标或原始 watcher 事件。后续截图和 issue 也必须遮盖这些信息。

## 自动化与 benchmark 证据

- [x] 2,000,000 baseline + 10,000 delta layered release benchmark：6 类查询最大 P95 423 µs（0.423 ms），满足 P95 ≤ 50 ms。
- [x] 10,000 条 durable batch release benchmark：commit 79,655 µs、journal 20,357 µs、layer apply 59,297 µs、query P95 14 µs。
- [x] 纯 2,000,000 baseline compact threshold：6 类查询最大单次 889 µs，满足每查询 < 250 ms；该组是单样本，不标记为 P95。
- [x] 三条 benchmark 命令均 exit 0，均报告 `1 passed`。
- [x] layered/batch 均为 baseline/overlay/tombstone = 2,000,000 或 100,000 / 5,000 / 5,000，估算 delta `20,177,198` bytes。
- [ ] 最终前端/Rust/格式化/lint/clippy/OpenSpec 完整门禁数字由主代理补录。

完整命令、逐查询数据、wall time、maximum RSS 和 peak memory footprint 见 `docs/large-index-performance.md` 的“2026-07-31 运行期增量 release 验证记录”。macOS `/usr/bin/time -l` 的 maximum RSS 与 peak footprint 覆盖完整 fixture 生命周期且计数口径不同，不等同 QuickFox ready 后常驻内存。

### 最终自动化待填（主代理）

> 待填：最终执行日期、commit、`npm run check` 或拆分命令、前端测试总数、Rust 测试总数、`openspec validate complete-runtime-incremental-indexing --strict`、`git diff --check` 的实际 exit code/摘要。阶段性参考值：前端 104、Rust 482。

## 发布阻塞项

| 阻塞项                   | 当前状态 | 负责人                 | 解除条件                                                              |
| ------------------------ | -------- | ---------------------- | --------------------------------------------------------------------- |
| macOS 真实桌面 watcher   | 阻塞     | macOS 发布验收维护者   | 在签名/发布构建上完成 macOS 清单全部 checkbox，附耗时、状态与 UI 截图 |
| Windows 真实桌面 watcher | 阻塞     | Windows 发布验收维护者 | 在 Windows + NTFS C:/D 上完成清单，附跨盘、断盘、junction、RSS 和截图 |
| 最终仓库门禁             | 待补     | 主代理                 | 补录最终命令与测试数字，全部 exit 0；若失败则记录问题并修复后重跑     |

任一平台缺记录、普通变化超过 watcher 到达后的 10 秒硬上限、事件风暴时 UI 无响应、恢复后索引不一致、路径泄漏或 Windows 内存超过硬上限，都不能解除相应 blocker。

## macOS 真实桌面结果

当前状态：**未执行，发布阻塞**。当前环境无法进行人类 GUI 交互，也未执行系统休眠、外接卷断开或真实 watcher failure 注入；没有状态截图，不能标记通过。

- [ ] create/write/rename/delete 与目录子树删除逐项记录可见耗时
- [ ] 持续事件流验证 5 秒静默窗口与首事件后 10 秒硬上限
- [ ] 1000-file Git checkout 验证 UI 响应、overflow 状态和最终收敛
- [ ] sleep/wake 验证 watcher 重启/校准和离线变化恢复
- [ ] root 断开/重连验证最近 baseline、dirty/Degraded 和恢复
- [ ] channel overflow、watcher 初始化/运行失败验证结构化 code 和恢复动作
- [ ] `content:` 准备中、非法查询、reader 不可用三类反馈
- [ ] 监听开关和 include/exclude 两阶段配置切换
- [ ] Watching、Calibrating、Degraded/fallback、恢复后 UI 状态截图
- [ ] 正文版本 lease/GC 的真实运行期行为与隐私检查

执行步骤和细分 checkbox 见 `docs/macos-manual-qa.md`。

## Windows 真实桌面结果

当前状态：**未执行，发布阻塞**。当前环境不是 Windows，未在 NTFS C:/D、任务管理器、睡眠/唤醒、盘符断开或 junction 上执行验收；没有状态/内存截图，不能标记通过。

- [ ] C: 与 D: 分别执行 create/write/rename/delete 和目录子树删除
- [ ] C:/D: 同盘跨目录 rename 与跨盘 move，验证旧/新路径不重复
- [ ] 1000-file Git checkout、channel overflow、watcher failure 和 10 秒硬上限
- [ ] sleep/wake、D: drive disconnect/reconnect 与盘符变化恢复
- [ ] UI 持续响应及 Watching/Calibrating/Degraded/fallback/恢复状态截图
- [ ] 任务管理器记录 baseline、风暴、校准、full refresh、2M ready 的 RSS/peak
- [ ] 真实 200 万文件级目标 < 500 MB、硬上限 < 800 MB
- [ ] 正文容量 8 队列、反馈、name/path 不回滚和版本 GC
- [ ] NTFS junction 越界、rename/delete 和外部目标不受影响的安全回归
- [ ] 配置两阶段切换、root 断开时最近 baseline 和结构化恢复动作

执行步骤和细分 checkbox 见 `docs/windows-manual-qa.md`。

## 解除 blocker 后的补录格式

每个平台至少补录：Windows/macOS 版本、QuickFox 构建类型与 commit、测试 root 类型、每个普通变化的 watcher 到达/结果可见耗时、1000-file 操作命令摘要、pending/dirty/code 变化、恢复耗时、UI/状态截图位置和内存观测。失败项必须附复现步骤、预期/实际结果和回归 issue；不得只写“通过”。

## 2026-08-09 自动化复验与 blocker 复核

- 已在 macOS 开发环境以 release synthetic fixture 复验 compact baseline、2,000,000 + 10,000 layered view 与 10,000 durable batch；结果见 `docs/large-index-performance.md` 的“2026-08-09 compact candidate 与增量 release 复验”。
- 2,000,000 compact 候选常驻估算为 `462,092,513` bytes；普通、短、数字和 fuzzy path 查询均由有界 posting 候选提供，未使用 `table.all_ids()` 作为常规回退。
- macOS 与 Windows 的本节所有未勾选手工项仍**未执行**。当前没有签名/发布构建的 GUI、真实 sleep/wake、外接 root 断开、NTFS C:/D、junction 或任务管理器截图证据；两个发布 blocker 保持有效。

## 2026-08-11 v1.6.0 发布决定

- 最终 `npm run check` 通过：313 个前端测试、520 个 Rust 测试通过，6 个显式
  release/benchmark 测试按设计忽略；Prettier、ESLint、前端构建、rustfmt 和 Clippy
  均 exit 0。
- `openspec validate --all --strict` 通过 8 个主规范，三个相关 change 已同步归档。
- macOS 开发版连接真实用户数据库验证了空文件 create/remove：分别持久化为
  generation 5 `upsert` 和 generation 6 `remove`，active baseline 未变化。
- 维护者确认设置页 tooltip 与新增文件搜索验收为 OK，并明确授权提交 `main`、发布
  v1.6.0，同时接受 Windows NTFS 多盘、休眠/唤醒和断盘恢复未形成新结构化手工记录
  的验证边界。该决定解除本版本发布阻塞，但不把未执行项目表述为已经测试通过。
