## Context

QuickFox 当前在 `build_runtime()` 中同步扫描默认索引目录，并把所有条目放进内存；搜索时对每个条目临时拼接 `name path` 并转换大小写。这个模型在小目录上简单可靠，但在 Windows 用户 profile 或 500GB 文件树上会导致启动慢、搜索卡顿，并且索引期间没有清晰的可用性反馈。

现有架构已经有 Rust core、SQLite storage、Provider registry、Tauri 托盘和 Shift+Shift 监听。设计应沿用这些边界：前端只展示和编辑配置，索引和 Provider 仍在 Rust core 内部，平台差异放在 Adapter 或编译配置中。

## Goals / Non-Goals

**Goals:**

- 启动 QuickFox 时不因文件索引阻塞主窗口、托盘或非文件 Provider。
- 文件索引可持久化，重启后能先使用上次完成的索引快照。
- 后台索引有明确状态，并能向前端暴露“未建立、建立中、可用、失败、使用旧索引”等信息。
- 搜索路径避免每次查询对全量条目重复分配和大小写转换，并尊重结果数量上限。
- Windows 发布构建常驻后台时不弹出 cmd/console 窗口；macOS/Linux 保持现有唤醒和托盘行为。
- 设置页提升为分区式控制台，并支持用轻量向导新增/编辑 DuckDuckGo 等网页搜索引擎。

**Non-Goals:**

- 不接入 Windows Search、Spotlight、Everything、locate 等系统或第三方索引服务。
- 不读取文件内容，不做全文检索。
- 不开放第三方插件 API。
- 不承诺在 macOS 开发机自动验证 Windows 全部桌面行为；无法自动化的部分进入手工 QA 文档。

## Decisions

### 1. 使用 SQLite 持久化索引快照，内存保存当前可搜索视图

新增 `index_entries` 表，记录 `path`、`name`、`kind`、预计算的 `search_text`、`updated_at_ms` 和索引批次。启动时先从 SQLite 读取最近完成批次，构造 `SearchIndex`；后台扫描完成后用事务写入新批次，再原子切换内存索引。

备选方案是只做内存优化并每次启动全量扫描。它改动小，但无法解决 Windows 大文件树启动慢，也无法在应用重启后复用结果。

### 2. 后台索引器由 Rust core 管理，前端只订阅状态

`QuickFoxRuntime` 增加 `IndexStatus` 和索引任务句柄/代次。启动时创建 runtime 后立即显示 UI，再启动后台索引线程。手动刷新和保存影响索引的设置时触发新的后台刷新；若已有刷新在跑，使用“最新请求胜出”的代次标记，旧任务完成后不覆盖新状态。

备选方案是在前端调用 `refresh_index` 并等待返回。它无法保证启动非阻塞，也会把索引生命周期泄漏到 UI。

### 3. 搜索在索引未就绪时降级，而不是阻塞

当没有可用索引时，文件 Provider 返回空结果和可选反馈项；计算器、网页搜索和命令 Provider 正常工作。当存在旧索引且后台刷新中，文件 Provider 使用旧索引并在 UI 显示“正在更新索引”。这样用户能继续使用 QuickFox，而不会被索引任务绑住。

### 4. 优化 `SearchIndex` 的查询结构

`IndexedEntry` 扩展或包裹为内部 `SearchableEntry`，缓存 lower-case `search_text`。普通查询先规范化一次，然后顺序扫描已缓存字段并在达到候选上限后停止继续构造结果；排序仍交给现有 Ranker，但 Provider 不再为每条记录重复 `format!` 和 `to_lowercase()`。

后续如仍不足，可在相同接口下增加 trigram/prefix 索引；本次先保持实现可测试、可维护。

### 5. Windows 无控制台窗口用编译子系统处理

在 Windows release 构建中使用 `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` 或等价 crate 入口配置，避免 GUI 应用常驻时弹出控制台。开发构建保留调试输出。macOS/Linux 不改变子系统，只保持托盘、窗口隐藏和 Shift+Shift 监听。

### 6. 设置页采用“分区控制台 + 向导弹层”

设置主界面按索引、网页搜索、历史、命令安全、外观/窗口分区，左侧或顶部提供稳定导航。新增/编辑网页搜索引擎使用轻量弹层，字段包括前缀、名称、URL 模板，并实时校验 `{query}`。内置示例包含 DuckDuckGo：`ddg` -> `https://duckduckgo.com/?q={query}`。

这吸收用户偏好的向导感，但避免把所有设置都变成多步流程。

## Risks / Trade-offs

- [Risk] 后台扫描仍可能占用磁盘 IO，影响 Windows 低配机器。  
  Mitigation: 分批写入、避免读取文件内容、保留排除规则，并在设置页显示状态和可手动刷新。

- [Risk] SQLite 索引迁移可能损坏或读取失败。  
  Mitigation: 迁移失败时不阻塞应用，状态进入失败并允许重建索引；配置和历史表保持独立。

- [Risk] keytap 在不同 OS 权限模型下行为不同。  
  Mitigation: 不改变热键核心逻辑，补充 macOS 权限提示和 Windows/Linux 手工 QA 项。

- [Risk] 旧索引可能显示已删除文件。  
  Mitigation: UI 标注后台刷新状态；刷新完成后替换快照。执行打开动作失败时仍由平台 opener 返回错误。

## Migration Plan

1. SQLite migration 新增索引条目和批次状态表；现有用户首次升级后会从空索引状态开始后台构建。
2. 启动时尝试加载最近完成索引；没有快照时文件搜索显示构建中或不可用提示。
3. 保存设置后持久化配置，并按是否影响索引决定触发后台重建。
4. 若索引表读取失败，保留应用可用性并允许用户从设置页重新索引。

## Open Questions

- 默认 DuckDuckGo 前缀采用 `ddg`；如果维护者希望更短的 `d`，可在实现前调整配置默认值。
- Windows 大文件树性能目标先以“不阻塞启动，搜索响应不因索引线程卡住”为第一标准；精确毫秒阈值需要 Windows 实机数据补充。
