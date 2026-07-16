# QuickFox 性能与体验深水区路线图设计

日期：2026-07-16  
状态：已完成对话设计确认，待维护者复核书面规格

## 1. 背景

QuickFox v1.5.0 已建立 200 万文件级 compact candidate index 和查询性能基线，但运行期文件变化与结果交互仍存在明显断点：

- `RuntimeIndexWatcher` 已能创建平台 watcher，但启动代码丢弃了事件接收端，文件变化没有进入索引更新链路。
- `SearchIndex::apply_update_batch` 虽能表达增删改，但每批都会重建完整 compact candidate index；百万级索引下不是真正低成本增量。
- SQLite 以完整 batch 保存索引快照，缺少可重放的增量 journal 与原子增量持久化。
- 计算器 Provider 已返回 `CopyText` 主动作，点击结果实际可以复制，但 UI 没有显式按钮、快捷键提示或成功反馈，用户难以发现和确认该行为。
- 前端仍根据 Action 内容推断显示文案，不足以稳定支撑不同结果类型的上下文动作。

本路线图面向开发者和重度电脑用户，以 macOS、Windows 为第一优先级，Linux 保持架构兼容并后续验收。产品交互坚持键盘优先，同时让鼠标操作完整且可发现。

## 2. 目标与非目标

### 2.1 目标

1. 自动增量索引默认开启，普通新建、修改、重命名和删除在 5 秒静默合并窗口后处理，并在最迟 10 秒内进入可搜索视图。
2. 手动刷新默认执行增量校准，只扫描确认可能变化的路径或子树；仅在明确条件下升级为全量重建。
3. 普通文件事件不重建百万级基线索引，通过只读 baseline、可变 overlay 和 tombstone 提供统一查询视图。
4. watcher 丢事件、溢出、权限失败、休眠唤醒或应用崩溃后可以恢复一致性，同时保留最近可用索引。
5. 建立统一的结果动作与反馈体系，覆盖计算器、文件、目录、内容片段和网页搜索的高频操作。
6. 提供仅本地的性能和索引诊断，默认不记录或导出完整路径、查询内容和文件正文，也不自动上传。
7. 维持现有 200 万 entry 查询性能目标，并为增量延迟、事件风暴、恢复和交互反馈建立回归门槛。

### 2.2 非目标

- 不接入 Spotlight、Windows Search、Everything 或其他第三方索引服务。
- 不开放第三方插件 API。
- 不进行无边界的启动器视觉重做；只调整服务于动作可发现性、反馈和诊断的组件。
- 不要求 Linux 与 macOS/Windows 在同一阶段完成全部真实桌面验收。
- 不把 watcher 当作唯一事实来源，也不承诺所有平台事件具有完全一致的原始语义。
- 不改变命令执行的安全确认边界；命令继续使用独立的 preview/确认界面。

## 3. 总体决策

采用分层增量架构：最近一次完成合并的 compact index 作为只读 baseline，新建和修改条目进入 delta overlay，删除和重命名前路径进入 tombstone。查询读取 `baseline + overlay - tombstones`，再交给现有 matcher 和 ranker 形成最终结果。

文件事件先进入有界 channel，由单一后台协调器消费。协调器负责 debounce、去重、事件风暴识别、局部扫描、journal 事务、内存视图切换、状态事件和后台 compaction。平台 watcher 只生产标准化事件，不直接修改索引。

体验侧由 Rust core 返回结构化 Action 描述，前端统一渲染主动作、快捷动作、更多动作、快捷键提示和反馈策略。前端不再通过比较路径或 Action 参数推断用户文案。

## 4. 增量索引架构

### 4.1 组件边界

#### Platform Watch Adapter

- 封装 `notify` 暴露的平台能力，把 create、write、remove、rename 和 overflow/failure 归一化为 QuickFox 事件。
- 只负责监听和发送事件，不扫描文件、不写 SQLite、不持有搜索索引。
- macOS、Windows 使用相同 core 协议，平台差异留在 Adapter 和集成测试边界。

#### Index Update Coordinator

- 持有事件接收端、debounce queue、dirty roots 和当前 generation。
- 普通事件使用 5 秒静默窗口合并，同一批最迟 10 秒提交，避免持续事件使更新永久饥饿。
- 对同一路径的 create/write/remove/rename 做确定性折叠。
- 负责将批次交给局部扫描器，并按“先持久化 journal，再切换内存视图”的顺序提交。
- 不持有 Tauri UI 逻辑，只通过结构化状态和事件向集成层报告。

#### Targeted Scanner

- 单文件变化只读取该路径的类型与轻量元信息。
- 新建目录或无法确定边界的目录变化扫描该局部子树，并继续应用现有强制排除、用户排除和项目 ignore 规则。
- 删除路径不访问文件系统，直接生成 tombstone；删除目录同时屏蔽其后代。
- 重命名规范化为旧路径 tombstone 与新路径局部扫描。
- 发现配置根目录变化、快照损坏或校准无法建立可信差异时，明确请求 full rebuild，而不是静默扩大扫描范围。

#### Layered Search Index

- `CompactBaseline`：最近一次完成 compaction 的不可变 compact index。
- `DeltaOverlay`：以规范化路径为键保存新增和修改条目，并维护只针对 delta 的小型候选结构。
- `TombstoneSet`：保存删除路径及删除目录前缀。
- `LayeredSearchView`：合并 baseline 和 overlay 候选，过滤 tombstone 和已被 overlay 替换的 baseline entry，最后交给既有 matcher/ranker。
- 查询期间使用同一个不可变 generation 视图，更新通过原子切换或短临界区完成，避免结果读取到半批状态。

#### Incremental Storage

SQLite 增加以下持久化概念：

- 当前 baseline snapshot 元数据及 schema version。
- 有序 delta journal：generation、operation、path、entry metadata、root、committed time。
- 每个 root 的 watcher/校准状态和最近成功游标。
- compaction 状态，保证新 baseline 完整写入后再原子设为 active。

启动时先加载 active baseline，再按 generation 重放未合并 journal。journal 重放必须幂等；同一批次重复执行不能产生重复 entry。旧的完整 batch schema 继续兼容读取，并在首次成功 compaction 后迁移到新结构。

### 4.2 正常数据流

1. Adapter 接收文件系统事件并写入有界 channel。
2. Coordinator 合并、去重事件；5 秒无新事件或达到 10 秒硬上限时形成 batch。
3. Targeted Scanner 为 changed paths 构建 entry，removed paths 生成 tombstone。
4. SQLite 事务提交 journal 和状态游标。
5. Runtime 原子切换新的 layered view generation。
6. Tauri 发出增量索引状态事件；前端按现有防抖与最新查询优先规则刷新当前查询。
7. 满足 compaction 条件时，在空闲期后台生成新 baseline，成功后原子切换并清理已合并 journal。

### 4.3 手动增量刷新

“立即增量刷新”执行一致性校准，而不是直接全量扫描：

- 优先处理已标记 dirty 的 roots 和未完成 journal。
- 比较 root/目录级校准信息，扫描确认可能变化的子树。
- 对新增、变化和缺失路径生成普通 delta batch。
- 仅在索引根目录配置改变、排除规则改变、快照或 journal 无法恢复、schema 不兼容、变化范围超过安全阈值时升级为全量重建。
- UI 必须明确显示“增量刷新”或“正在全量重建”及触发原因。

### 4.4 事件风暴与 compaction

Coordinator 的队列必须有上限。批量 Git checkout、依赖安装或解压产生事件风暴时，不逐项无限追赶：

1. 合并已收到的事件。
2. 将受影响 root 标为 dirty。
3. 丢弃该 root 后续可被校准覆盖的重复事件。
4. 在系统空闲时执行局部校准。

Overlay 达到配置化条数或内存软阈值，或者 journal 存续超过时间阈值时，安排后台 compaction。阈值先由 benchmark 决定，不在设计阶段固定未经测量的常数。compaction 不阻塞查询；失败时继续使用旧 baseline 与现有 overlay，并保留可重试状态。

### 4.5 一致性与降级

- watcher 是低延迟信号，SQLite baseline、journal 和校准结果才构成可恢复事实。
- watcher 初始化失败时保留文件搜索，状态标记为“自动增量已降级”，并允许手动增量刷新。
- watcher 运行期 overflow、channel 满、休眠唤醒或监听重启会把相关 root 标为 dirty。
- 路径暂时无权限或文件在扫描时再次变化时，记录可重试 failure，不删除最近仍可信的 baseline entry；明确收到 remove 或校准确认缺失时才删除。
- 内容索引更新失败不影响 name/path delta 提交；entry 记录内容索引失败状态，后续独立重试。
- 所有状态错误必须带机器可读 code 和用户可读摘要，禁止仅输出日志后静默失效。

## 5. 统一结果动作与反馈

### 5.1 Core Action 描述

在现有可执行 `Action` 之外增加面向展示的结构化描述，例如：

- `id`：结果内稳定动作标识。
- `label`：由 core/provider 按上下文给出的显示文案。
- `action`：现有 Rust core 可执行 Action。
- `placement`：`primary`、`quick` 或 `menu`。
- `shortcut_hint`：可选的跨平台快捷键语义，由前端适配为 `⌘` 或 `Ctrl` 展示。
- `feedback`：成功反馈种类与安全摘要；失败反馈默认使用 Action 执行错误。
- `dismiss_policy`：成功后隐藏、保持窗口或由结果类型决定。

Provider 仍只产出结果和 Action，不直接执行动作。Action 执行继续统一经过 Rust core 和平台 Adapter。

### 5.2 前端交互规则

- `Enter` 始终执行当前结果主动作。
- `⌘/Ctrl + Enter` 执行第一个快捷动作；只有存在快捷动作时才展示提示。
- `Tab` 或右方向键可从搜索结果进入动作区，Esc 返回结果导航。
- 鼠标悬停或键盘选中结果时显示最多两个高频快捷按钮；其余动作进入“更多”菜单。
- 点击结果空白区域执行主动作；点击按钮必须阻止事件冒泡，不能同时执行主动作。
- 右键继续打开完整动作菜单，保持现有重度用户习惯。
- 计算器结果显式显示“复制结果”，并保持 Enter 复制。
- 文件/目录优先显示与上下文最相关的快捷动作，例如复制路径；内容命中可显示复制片段；网页结果可显示复制链接。
- 命令结果继续使用独立 preview/确认模式，不显示为普通快捷动作。

### 5.3 反馈规则

- 复制成功显示轻量 Toast，例如“已复制 ‘960’”；长内容只显示安全摘要。
- 同类连续 Toast 合并或替换，不阻挡输入和结果导航。
- 打开类动作成功后按结果的 `dismiss_policy` 处理窗口；失败时保留查询和选中项。
- 失败在结果附近或统一反馈区显示可解释原因，并在适用时提供“重试”或“打开设置”。
- Toast 和错误反馈使用 `aria-live`，键盘用户无需移动焦点即可获知结果。

### 5.4 前端组件边界

本变更触及结果列表时，应从当前大型 `App.tsx` 中提取清晰边界：

- `ResultList`：选择与列表渲染。
- `ResultItem`：标题、路径、snippet 和动作区布局。
- `ResultActionBar`：快捷动作与键盘焦点。
- `ActionMenu`：完整动作菜单。
- `ActionFeedback`：Toast、错误与 `aria-live`。

组件只接收类型化 props，不直接调用 Tauri；执行函数由 launcher controller 注入，便于测试且符合前后端边界。

## 6. 本地诊断与隐私

设置页索引区域增加可折叠“运行状态”，至少展示：

- 自动增量开关与 watcher 状态。
- 已监听 root 数量，不默认展示完整 root 路径。
- 待处理事件数、dirty root 数和最近批次条目数/耗时。
- baseline、overlay、tombstone 的条目数和估算内存。
- 最近 compaction、校准和恢复结果。
- 查询延迟的本地滚动统计，例如 P50/P95。
- 当前降级原因和建议操作。

用户可以导出脱敏诊断 JSON。默认导出不得包含完整路径、原始查询、文件正文、内容片段、用户名称或机器标识；如后续需要包含敏感细节，必须提供单独的显式选项和导出前预览。本路线图不增加任何自动上传。

指标使用有界内存窗口或聚合计数，不能因长期运行无限增长。诊断写入失败不影响索引和查询主流程。

## 7. 性能与体验门槛

### 7.1 自动化门槛

- 普通单文件变化在事件到达后最迟 10 秒进入查询视图。
- 200 万 deterministic entry 的普通 name/path 查询继续满足 P95 小于等于 50ms；低命中查询不退化到全量 matcher。
- 小型 overlay 不得让查询延迟相对 baseline-only 基线产生不可解释的数量级退化。
- 事件风暴测试证明队列有界，并转为 dirty-root 校准，而不是无限积压。
- journal 在事务中断、重复重放和 compaction 切换失败后仍能恢复一致视图。
- rename、目录删除、overlay 覆盖 baseline、tombstone 前缀过滤和内容索引失败均有单元/集成测试。
- Action 契约、按钮事件不冒泡、键盘/鼠标等价语义、Toast 合并、失败保留现场和无障碍反馈有前端测试。

### 7.2 macOS/Windows 发布验收

- 普通新建、修改、重命名、删除在真实目录中按目标时间可搜索或消失。
- 批量 Git checkout、依赖安装或大目录移动不会让 UI 无响应或无限增长内存。
- 休眠/唤醒、外接盘断开/重连、权限拒绝和 watcher 重启后状态可解释，并能通过校准恢复。
- 运行 compaction 时启动器输入、计算器和文件搜索保持响应。
- 计算器、复制路径、复制内容片段和复制链接均有可见反馈；鼠标和键盘路径一致。
- 脱敏诊断导出不包含用户完整路径、查询内容或文件正文。

Linux 先运行可自动化 core 测试和 CI 构建，真实 watcher 与桌面交互验收在后续平台阶段补齐。

## 8. 分阶段 OpenSpec 路线图

### 阶段 0 + 1：完成运行期增量索引

建议变更名：`complete-runtime-incremental-indexing`

- 先用失败测试证明 watcher 接收端断链和现有大索引 batch 重建问题。
- 建立 coordinator、targeted scanner、journal、overlay、tombstone 和 layered search view。
- 自动增量默认开启，手动刷新切换为增量校准，并保留明确的 full rebuild fallback。
- 加入必要的本地指标与状态 code，为后续诊断 UI 提供稳定 contract。
- 完成崩溃恢复、事件风暴和 macOS/Windows watcher 验收。

### 阶段 2：统一结果动作与反馈

建议变更名：`unify-result-actions-and-feedback`

- 扩展结构化 Action 描述，消除前端文案推断。
- 提取结果列表、动作区、菜单和反馈组件。
- 覆盖计算器、文件/目录、内容片段、网页链接和失败重试。
- 完成键盘、鼠标、无障碍和视觉 QA。

### 阶段 3：深水区诊断与后台维护

建议变更名：`add-local-index-diagnostics-and-compaction`

- 根据阶段 1 benchmark 确定 overlay compaction 阈值。
- 完成空闲期 compaction、校准策略和设置页本地诊断。
- 固化查询、内存、事件风暴、恢复和平台发布门槛。
- Linux 补齐真实 watcher 与桌面交互验收。

三个变更分别验证和归档，不能以一个长期开放的超大 change 代替。阶段 2 可在阶段 1 的 core contract 稳定后并行准备，但不能共享未稳定的数据模型编辑。

## 9. 风险与缓解

- **分层查询可能产生重复或排序漂移。** 使用旧 baseline 查询作为 oracle，增加 overlay 覆盖、tombstone 过滤和稳定排序测试。
- **目录 rename/delete 的前缀过滤成本可能升高。** 使用规范化路径和前缀结构；真实结构由 benchmark 在实现计划中选择。
- **journal 增加 schema 与恢复复杂度。** 所有写入事务化、重放幂等、baseline 切换原子化，并准备旧快照迁移 fixture。
- **watcher 跨平台语义不同。** Adapter 归一化、事件只作为提示、校准负责最终一致性。
- **后台 compaction 与查询争用 CPU/内存。** 只在空闲期运行、设置资源预算、允许取消，并始终保留旧 baseline。
- **显式按钮可能让列表变拥挤。** 仅选中/悬停项显示最多两个快捷动作，窄窗口降级为一个快捷动作加更多菜单。
- **诊断信息可能泄露隐私。** 默认聚合、路径脱敏、查询与正文不采集、无自动上传。

## 10. 验收结论

本路线图的产品判断标准不是“索引功能存在”或“动作可以执行”，而是：用户新增文件后不需要理解索引内部机制；常用动作无需猜测且结果立即可感知；系统遇到事件风暴、权限、休眠和崩溃时仍保持可用并能说明当前状态；性能与体验均有可重复验证的发布门槛。
