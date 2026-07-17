## Why

QuickFox 已具备运行期 watcher、批处理类型和大规模 compact 搜索索引，但 watcher 的事件接收端没有进入运行时消费链路，现有批量更新还会重建完整候选索引并保存完整 SQLite batch。因此用户新建文件后仍需等待昂贵刷新，百万级索引下无法提供可信、低成本的自动增量体验。

## What Changes

- 接通平台 watcher 到 Rust core 的有界事件消费链路，自动增量默认开启，并以 5 秒静默窗口、10 秒硬上限合并普通文件变化。
- 引入针对单文件和局部子树的 targeted scanner；删除与重命名不再要求扫描完整配置范围。
- 将运行期搜索视图拆成只读 compact baseline、可变 delta overlay 与 tombstone，普通增量批次不重建百万级 baseline。
- 在 SQLite 中持久化可幂等重放的 delta journal、active baseline 和 root 校准状态，支持崩溃恢复和原子 baseline 切换。
- 将手动刷新默认改为增量校准；只有配置语义变化、持久化状态损坏或无法建立可信差异时才升级为全量重建，并暴露升级原因。
- 对 watcher 溢出、事件风暴、权限失败、休眠唤醒和 channel 满提供 dirty-root 校准与可观察降级，始终保留最近可用索引。
- 增加 delta 延迟、队列/overlay 有界性、journal 恢复和 layered query 性能回归，以及 macOS/Windows 真实 watcher 验收要求。

## Capabilities

### New Capabilities

无。本变更完成并强化现有索引、配置和性能能力，不新增独立产品能力域。

### Modified Capabilities

- `search-index`: 将现有运行期监听、手动增量刷新、持久化快照和状态反馈要求收紧为真正的局部更新、分层查询视图、journal 恢复与可解释降级。
- `configuration-and-history`: 自动增量索引默认开启，并允许用户在设置中关闭或重新开启；影响索引语义的配置变化明确触发全量重建。
- `large-scale-search-performance`: 为 200 万 entry 场景增加增量可见延迟、overlay 查询开销、事件队列与增量状态内存预算。
- `project-quality`: 增加 macOS/Windows 真实文件变化、事件风暴、休眠/唤醒和恢复路径的发布验收记录。

## Impact

- Rust core：`index_watcher`、`index_scanner`、`index`、`compact_index`、`storage`、`config` 和 Tauri runtime 生命周期。
- 前端/Tauri contract：索引状态增加自动增量、pending/dirty、最近批次和降级原因等结构化字段；设置页沿用现有 watcher 开关并显示基础状态。
- SQLite：新增 baseline/journal/root-state schema，并兼容读取现有完整 batch 快照。
- 测试与文档：新增 core 单元/集成/性能测试，更新 macOS/Windows 手工 QA 和索引故障排查文档。
- 不改变 Provider/Action 安全边界，不引入外部索引服务，也不要求第三方插件 API。
