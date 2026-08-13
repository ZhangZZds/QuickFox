## Context

问题实例中 `quickfox.sqlite` 已达到 17.5 GB，回滚 journal 超过 1 GB 且持续变化。
当前 `save_completed_index_batch` 在一个事务中插入全部 entry；默认 rollback journal
会随被修改的 B-tree 页面增长。`index_batches/index_entries` 保存多个完整快照，激活
新 baseline 后没有清理旧批次。Windows 首次配置还把所有可访问盘符根目录放入
`include_dirs`，而应用没有单实例限制。

## Decisions

### 单实例先于应用初始化

将 `tauri-plugin-single-instance` 注册为第一个 Tauri 插件。第二个进程发现已有实例后
退出，并把事件交给主实例；主实例在 UI 线程显示、取消最小化并聚焦 launcher。

### 安全的首次范围

Windows 新配置只选择实际存在的用户热路径；如果一个都不存在才使用整个用户
profile。盘符发现仍保留给用户显式选择 `complete` 或自行配置大根目录的扫描计划，
但不再作为首次默认值。

启动时若现有索引配置与旧版“所有可用盘符 + 其余字段默认值”完全一致，将其迁移为
新的安全范围并原子保存；任一索引字段被修改都视为显式配置，不自动迁移。

### 可恢复的分块 baseline

`index_batches.status` 区分 `building` 与 `completed`。先用短事务创建 building 行，
每 2,048 条 entry 提交一次，最后用短事务标记 completed。所有恢复和 latest 查询只
读取 completed；启动时分块删除残留 building 数据。因此任一 SQLite journal/WAL
事务都有明确上界，而进程崩溃不会暴露半个 baseline。

### 保留和空间策略

最终 baseline 激活后只保留 active baseline。阶段 checkpoint 保存后只保留 active
baseline 和最新 checkpoint，避免失败/重试无限累积。每次删除以 4,096 行为一批，
随后请求 incremental vacuum。新数据库启用 incremental auto-vacuum；旧数据库不
自动运行 `VACUUM`，因为它可能临时需要接近数据库大小的额外空间。

持久化前按字符串字段、规范化键、数据库索引和行开销做保守估算。估算超过 8 GiB，
或可用空间不足“估算值 + 5 GiB 保留空间”时终止写入。该保护优先保证系统盘可用，
索引失败沿现有状态路径展示，原始文件不受影响。

## Risks / Trade-offs

- WAL 的 `journal_size_limit` 只约束 checkpoint 后保留大小，不是活动 WAL 的绝对硬
  上限；分块事务和自动 checkpoint 才是限制峰值的主要手段。
- Windows 默认范围变窄，首次搜索覆盖率降低；用户仍可在设置中添加 D 盘的具体数据
  目录，避免直接添加盘符根目录。
- 删除旧 SQLite 行不会让未启用 auto-vacuum 的历史数据库立即缩小；为避免磁盘翻倍，
  不自动对大旧库执行 `VACUUM`。
- baseline 空间估算是安全上界而非精确文件大小，极端长路径数据可能更早触发保护。

## Verification

- 单元测试覆盖 building 批次不可见及重启清理、旧批次保留策略、WAL/auto-vacuum
  策略、空间预算和 Windows 默认范围。
- 完整 Rust 测试、rustfmt、Clippy、前端检查和 OpenSpec strict validation。
- Windows 手工验收覆盖重复启动、首次无全盘扫描、数据库/WAL 峰值和低空间失败提示。
