# 运行期索引校准屏障设计

## 背景

现有实现已经能恢复 baseline 与 committed journal，也已经为刷新建立 successor watcher，
但仍有四类竞态：启动阶段同步校准阻塞 Tauri setup；watcher 注册与校准之间依赖固定
500ms 等待；baseline 激活会用扫描时清单覆盖 activation 前已提交的目录变化；缺失 root
监控线程没有可取消、可 join 的生命周期。配置 revision 的 storage 读取失败和 baseline
持久化失败还没有进入同一个恢复漏斗，可能留下磁盘配置与运行时服务不一致。

## 目标

- Tauri setup 只恢复 baseline+journal 并返回；所有 root 枚举和校准在 setup 返回后的 worker 中执行。
- watcher 先于权威校准建立，校准完成并 drain 捕获事件后才宣告 `Watching`。
- 配置 revision 只有在新 session 的 capture、calibration 和 generation fence 全部成功后才提交磁盘与内存配置；失败保留旧 service/view。
- baseline activation 使用同一 generation 的 entries 与 manifest 权威快照，不覆盖 activation 前已经 durable 的 manifest 变化。
- root monitor 与所有恢复重试有显式取消、join 和单 revision latch；失败统一进入 Degraded/恢复漏斗。

## 决策

### 1. Capture-calibrate session

core 增加纯状态的 `RuntimeCalibrationSession`。session 阶段为 `Preparing`、
`Capturing`、`Calibrating`、`Fenced`、`Watching`、`Degraded`。Tauri 层负责具体
watcher、scanner、storage 和 main-thread dispatch，core 只决定阶段转换、revision fence
和恢复 latch。

启动构造函数只调用 `recover_layered_index`。setup 返回后排队 worker；worker 使用
`build_scan_plans` 得到显式、applications、user-hot-paths 和 remaining-drive roots，并与
当前 layered index roots 去重。worker 先创建 watcher/coordinator，再逐 root 做权威校准，
最后停止并 drain 临时 capture 到一个 generation fence，然后安装正式 service。校准期间
baseline+journal view 始终可查询，状态保持 `Preparing`；任何失败保留旧 view 并置
`Degraded`。

### 2. 配置 revision 两阶段提交

`save_config` 先构建候选 session，不写 config store。旧 runtime service 在整个候选
capture+calibration+fence 阶段继续工作。候选成功后，在 refresh fence 下：

1. join 旧 service，读取其最终 committed generation；
2. drain 候选 capture 并提交校准/tail；
3. 保存 config store；
4. 原子替换内存 config、index view 和正式 service。

任何 watcher、scanner、storage、config persistence 或 dispatch 错误都中止候选，停止
候选 handle，保留旧 config/service/view，并通过统一 failure application 返回明确错误。
不再使用固定 sleep 猜测 watcher 是否完成注册。

### 3. Manifest activation 静止切换

不增加 schema。activation 前先注册 standby watcher，随后停止并 drain 当前 durable
successor。把所有 committed tail 同时重放到扫描 entries 与扫描 manifest，得到同一
`authoritative_generation` 的权威快照。SQLite 事务可以继续完整替换 manifest，因为写入
的是已吸收 activation 前全部 durable 变化的最终清单；事务成功后再把 standby watcher
接成 durable successor。

若进程在 activation 事务窗口崩溃，standby 内存事件可能未 durable，但重启状态只能是
`Preparing`/`Degraded`，且 watcher-first 权威校准会对真实文件系统收敛。集成测试直接
覆盖目录创建和删除跨越 activation、模拟崩溃并重新走生产恢复入口。

### 4. Managed root monitor 与统一失败漏斗

root monitor 由 core 生命周期类型拥有 cancellation token 与 `JoinHandle`。创建、配置
revision 变化、root 恢复、应用退出分别执行 start、stop+join。spawn、probe、main-thread
dispatch 失败都会清理 retry latch，并返回统一 `RuntimeFailureApplication`：保留 live
view、状态置 `Degraded`、同 revision 最多安排一次恢复。

baseline persistence `Failed` 也使用同一漏斗，即使已经存在 no-op successor，也必须
停止该 successor、恢复 live service 或安排一次校准 session，不能永久停在无服务状态。

### 5. Watcher registration 与 ContentIndex 生命周期

QuickFox 先利用 notify 8.2.0 原生 backend 的同步 `watch()` 边界：Linux inotify 与
Windows ReadDirectoryChanges 都等待 backend command reply。macOS FSEvents 的 command ack
不足以保证紧随其后的 mutation 可见，因此在所有用户 roots 注册后，最后向同一个原生
watcher 注册应用私有临时目录；只在该目录循环写 registration probe，直到同一 callback
实际观察到 probe event 才返回。probe 不写入用户 root，并与 watcher 同生命周期回收。
probe 目录必须由 `tempfile::Builder` 原子随机创建并持有 `TempDir` ownership，禁止使用可预测
名称配合手工递归删除，避免预置 symlink 把清理重定向到非 QuickFox 目录。
升级 notify 时必须重新审计三个平台的同 stream 契约。可观察 ack 后仍执行权威 calibration
并 drain inbox；测试在 `watch_roots` 返回后的下一条语句制造 mutation，不使用固定 sleep。

ContentIndex 不再复用并删除共享 `content-v1` 目录。每次 build 写入独立版本目录，
SearchIndex 原子发布新版本；旧 ContentIndex 的最后一个 reader 释放后，由目录 lease 回收
旧版本。启动恢复只安装 name/path view 并把 content 标记为未构建，setup-return gate 释放后
由后台 worker 构建并原子安装正文索引。版本目录同样由随机 `TempDir` 创建，带 QuickFox
marker，并登记到进程内 active registry；lease 还必须持有 marker 的跨进程文件锁，GC 只有在
非阻塞取得该锁后才可回收，避免第二个 QuickFox 进程删除仍在使用的版本。每次 build 和启动
worker 只回收不在 registry 中、名称和真实普通 marker 文件都匹配的 crash orphan，不处理
symlink、未标记目录或其他文件。正文索引成功安装后发布一次 `quickfox://index-status`，让当前
查询自动重跑；superseded build 不发布。

正文查询的语法错误与 reader/search I/O 错误使用结构化错误边界。前者返回可操作的非法查询
feedback；后者返回内容索引不可用 feedback，同时把 runtime 标记为 degraded 并发布状态，
不得把任何错误折叠成空结果。

## 被拒绝的方案

- 为 manifest 新增 generation/tombstone journal：允许 activation 与 durable writer 并行，
  但本轮需要额外 migration 和恢复协议；静止切换能以更小范围满足一致性要求。
- 在用户索引 root 写隐藏 probe：会引入权限、同步软件和用户可见副作用；registration probe
  只允许写入应用私有临时目录，并复用同一个原生 watcher stream。

## 验证

- 单元测试覆盖 session 阶段、revision fence、retry latch、monitor cancellation/join。
- 生产入口测试使用可控 watcher/scanner/storage/spawner，不使用固定 sleep，实际交错事件。
- 集成测试覆盖 activation 前目录 create/delete、activation、模拟 crash、重新启动校准。
- 完整执行 Rust tests、rustfmt、clippy、前端 tests/check；不修改 OpenSpec tasks。
