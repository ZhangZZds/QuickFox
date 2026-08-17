# QuickFox 排错指南

## `npm run tauri dev` 启动但窗口背景不透明

macOS 透明窗口需要同时满足：

- `tauri.conf.json` 中启用 `app.macOSPrivateApi`
- `Cargo.toml` 中为 `tauri` 启用 `macos-private-api` feature

否则会看到整块窗口背景而不是紧凑启动器效果。

## 命令模式只有提示，没有真正执行

先确认两件事：

- 查询以 `>` 开头
- 配置中 `command.enabled = true`

如果仍然没有执行，检查：

- 命令是否被安全规则拦截
- 外部终端是否可用
- macOS 是否允许 Terminal 被脚本唤起

## Linux 终端找不到

Linux 侧按如下顺序尝试终端：

1. `x-terminal-emulator`
2. `gnome-terminal`
3. `konsole`
4. `xfce4-terminal`
5. `xterm`

如果都不存在，命令执行会失败。请安装其中一个终端，或后续扩展首选终端配置。

## Windows Terminal 行为异常

当前 Windows 命令构造默认走：

```text
wt.exe cmd.exe /C <command>
```

若运行失败，请检查：

- `wt.exe` 是否已安装并在 PATH 中
- Windows Terminal 是否被系统策略限制

## 索引刷新后没有结果

优先排查：

- `index.include_dirs` 是否为空
- 目录是否可读
- 是否被 `exclude_dirs` 或 `exclude_patterns` 排除

可以通过手动刷新后的失败报告定位不可读目录。

## 保存索引设置后模式又变回旧值

设置页现在区分“配置保存”和“索引应用”：

- “保存失败”表示配置文件没有写入，应先检查配置文件目录权限或磁盘状态。
- “配置已保存，索引正在后台应用”表示新模式已经持久化，可以关闭并重开设置；后台仍在建立新 revision。
- “部分索引位置不可用”表示其他可用位置已经应用。恢复离线盘符或目录权限后，点击“重试/校准索引”。
- “索引应用失败”不会把 `balanced`、`complete` 或目录列表改回旧值；最近可用索引会继续服务。

Windows 新配置默认覆盖当前可用盘符，`balanced` 会先提供应用入口和用户热路径结果，再在后台补全全盘。`Windows`、`ProgramData`、`PerfLogs`、恢复/升级目录、回收站、卷元数据、`AppData` 和虚拟内存文件会自动跳过，`Users` 下的普通文件不会被系统排除规则跳过。

全盘补全可能需要较长时间。单个不可访问、临时离线或读取失败的盘符/目录不会拒绝配置保存或撤销其他盘符；状态会显示 `partial`，恢复位置后点击“重试/校准索引”。若重开设置确实显示旧值，请记录设置页的保存错误，而不是删除索引数据库。

## 自动增量没有反映文件变化

先在设置页“搜索与索引”确认：

- “运行期文件监听”已开启。
- 自动增量状态已经从“正在准备”进入“运行中”；`Preparing` 或 `Calibrating` 时先等待当前校准结束。
- 文件位于 include root 内，且未被 exclude root、exclude pattern 或 project ignore 排除。
- 对应 root 当前存在且可读；移动磁盘、网络盘或外接盘已经重新挂载到原位置。

普通变化的 10 秒门槛从 watcher 事件到达 coordinator 开始。请分别记录文件操作时间、状态中最近批次时间和结果可见时间；操作系统延迟交付事件时，不能把两者混为一项。

恢复顺序：

1. 等待状态中的 pending events 归零；事件风暴后等待 dirty-root 校准完成。
2. 点击一次“刷新索引”。它会先提交待处理事件并执行 manifest 增量校准。
3. root 曾断开时，先恢复原挂载位置，再手动刷新；断开期间最近 baseline 结果可能继续可见，避免把暂时不可达误判为删除。
4. 仍未恢复时记录结构化 code、pending/dirty 数、最近批次条目与耗时，并保留状态截图。不要在 issue 中粘贴个人完整路径。

关闭自动增量不会删除 baseline 或 journal，普通文件搜索和手动刷新仍可用。重新开启后必须先完成 root 校准，才会显示“自动增量运行中”。

## 自动增量降级 code 与恢复动作

| code                          | 含义                                 | 建议恢复动作                                                                |
| ----------------------------- | ------------------------------------ | --------------------------------------------------------------------------- |
| `watcherInitializationFailed` | watcher 启动或 root 注册失败         | 检查 root 是否存在/可读，恢复权限或挂载后重新开启监听或手动刷新             |
| `watcherRuntimeFailed`        | 已启动 watcher 在运行中中断          | 保留应用运行，恢复 root/权限，等待自动恢复；必要时手动刷新                  |
| `watcherOverflow`             | 平台报告事件丢失或 rename 不确定     | 等待 dirty-root 校准；若升级为全量刷新，保持旧结果可用直到完成              |
| `channelOverflow`             | 8192 事件通道或 pending 集合达到上限 | 停止制造事件风暴，等待校准；再检查批量操作后的最终结果                      |
| `journalWriteFailed`          | committed journal 无法写入           | 检查磁盘空间和索引存储目录权限；修复后手动刷新，失败批次不会推进 generation |
| `journalReplayFailed`         | 启动时 journal 无法可信重放          | 继续使用最近 baseline，等待/触发带原因的后台完整刷新                        |
| `calibrationFailed`           | manifest 校准无法建立可信差异        | 修复不可读/缺失 root，手动刷新；持续失败时执行后台完整刷新                  |
| `fullRefreshFallback`         | 增量状态不可恢复或达到安全阈值       | 让后台完整刷新完成；期间不要删除本地索引文件，旧搜索视图仍可用              |

overlay+tombstone 达到 50,000 条或估算增量状态达到 64 MiB 会主动显示 fallback 并安排 baseline 刷新，这是有界资源保护，不代表已有 baseline 丢失。

## 批量 checkout、休眠或磁盘断开后状态不一致

- 1000 文件级 Git checkout、大目录移动或依赖安装可能触发 overflow。停止批量操作后等待 `pendingEvents = 0`、`dirtyRoots = 0`，再抽查创建、覆盖、重命名和删除结果。
- 睡眠/唤醒后若显示 watcher 中断或校准中，保持 QuickFox 运行并等待恢复；超过一轮校准仍不一致时手动刷新。
- root 断开时系统保留最近可用 baseline 和未确认删除的条目。重连同一路径后手动刷新；不要通过删除 SQLite/Tantivy 目录来“清缓存”。
- Windows 盘符改变时，原 root 和新盘符是不同配置语义。先确认盘符，再在设置中修改 root；配置会先持久化，后台应用失败时保留新设置和最近可用索引，并显示可重试状态。

## 保存索引配置失败或一直使用旧范围

include/exclude root、排除模式、project ignore 和正文范围使用两阶段配置切换。新 candidate watcher、校准或持久化任一步失败时，磁盘/内存配置、旧 service 和旧查询视图都保持不变；这属于一致性保护。

修复失败摘要指出的 root、权限或存储问题后重新保存。不要连续切换配置规避校准，也不要手动编辑 SQLite generation/journal 表。

## `content:` 查询没有结果

- “内容索引仍在准备”：name/path 已可用，等待后台正文版本安装；成功后状态事件会让当前查询自动重跑。
- “无效内容查询”：检查 `content:` 后的 Tantivy 查询语法；该错误不会让运行时降级。
- “内容索引查询失败”：reader/search I/O 不可用，name/path 仍可用；系统会标记降级并安排恢复。
- 文件超过配置的正文大小限制（默认 2 MiB）、是二进制或不支持的富文档时，只跳过正文，文件名/路径仍应可搜索。

正文增量使用容量 8 的有界队列和单 worker，避免并发发布乱序。队列暂时满不会回滚 name/path 批次；系统转为后台恢复。正文版本目录由 lease 与 marker 锁管理，旧 reader 释放后自动回收。不要手工删除 `content-v1/build-*`，否则可能破坏正在使用的 reader 或另一个 QuickFox 进程。

## 全局快捷键不符合预期

当前仓库已经有双击 Shift 状态边界和窗口显隐边界测试，但真实桌面行为仍需平台手工验收。

若出现异常，请记录：

- 平台与系统版本
- 是否在前台应用中被其他软件占用
- 触发时窗口是否已存在但未聚焦
