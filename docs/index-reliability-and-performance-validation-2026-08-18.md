# 索引可靠性与性能迭代验收记录（2026-08-18）

本文记录 `index-reliability-and-performance-iteration-plan.md` 的工程交付和自动化验收结果。验证环境为 macOS 26.5（25F71）、arm64；Windows NTFS 实机数据单独列为发布前验收项，不用 macOS 或合成数据代替。

## 工程交付

| 工作包               | 结果 | 主要实现                                                                                                                                |
| -------------------- | ---- | --------------------------------------------------------------------------------------------------------------------------------------- |
| 索引代际原子化       | 完成 | schema v5；`building -> prepared -> active -> obsolete`；唯一 active/pending；配置、manifest、runtime generation 和 active 切换共用事务 |
| 崩溃恢复             | 完成 | building 按目录 checkpoint 续扫；prepared 重启只恢复 Preview 并继续 finalization；相同配置 active 直接恢复                              |
| 搜索连续性           | 完成 | 查询视图合并 Active、Root Preview 和增量层；Preview 在新 Active 成功安装后才释放，finalization/activation 失败继续可搜索                |
| 启动链路             | 完成 | 同步阶段只创建最小 Runtime；SQLite Active 恢复、校准、GC 和 vacuum 迁移在 setup 后后台执行                                              |
| SQLite 治理          | 完成 | 孤儿/obsolete 回收、有界 WAL checkpoint、incremental auto-vacuum、新旧库迁移状态和缓存诊断                                              |
| Windows 扫描 Adapter | 完成 | 统一 `IndexSource`；固定 NTFS 使用无服务 Win32 批量枚举；USN 能力探测；Generic fallback；目录 checkpoint、进度和取消                    |
| Watcher 降级         | 完成 | 根目录祖先覆盖压缩、逐根 watcher、dirty-root 局部恢复；活跃全量刷新吸收同 revision dirty 事件                                           |
| 状态与 UI            | 完成 | 七阶段 `phase`、八类 `refreshReason`、存储诊断、后台降级非阻塞提示、设置页全视口自适应                                                  |
| 隐私边界             | 完成 | 诊断序列化不包含配置指纹、generation root 路径或原始 watcher 事件                                                                       |

## 自动化验证结果

最终统一检查：

```text
npm run check
Prettier: passed
ESLint: passed
Vitest: 8 files, 339 tests passed
TypeScript/Vite production build: passed
rustfmt: passed
Clippy --all-targets -D warnings: passed
Rust tests: passed (6 platform/performance tests ignored by default)
```

新增和既有回归覆盖：

- building 目录断点续扫、prepared 重开和幂等完成；
- 激活事务失败保留旧 Active，成功后只有一个 Active；
- 连续 10 次兼容重开不创建新 generation；
- GC 只保留 active 和最新 pending，并报告删除条目、freelist 和完成时间；
- 旧 `auto_vacuum=NONE` 数据库识别及后台迁移；
- Root Preview 在 prepared、finalizing 和 activation 失败后持续可搜索；
- 启动 Runtime 不同步恢复 SQLite，setup gate 后才执行后台恢复；
- watcher 根覆盖压缩、单根失败隔离和全量刷新期间 dirty-root 吸收；
- Windows 路径规范化、父根去重、Generic 流式批次、取消和安全降级；
- 前端阶段、刷新原因、真实可用性、非阻塞 degraded 状态和响应式布局。

macOS 当前运行环境的原生 FSEvents watcher 不返回注册确认事件。测试构建因此使用 notify 的 kqueue 后端验证真实文件事件语义；生产 macOS 仍使用 RecommendedWatcher/FSEvents。串行全量测试已验证 kqueue 路径无失败。

## 200 万条目搜索基线

使用 release 构建运行 2,000,000 条目阈值测试和内存基线：

| 查询                      | 耗时      |
| ------------------------- | --------- |
| `agents-exact`            | 40 µs     |
| `agents-prefix-extension` | 1 µs      |
| `agents-prefix`           | 1 µs      |
| `agents-type-md`          | 51,621 µs |
| `agents-dir-workspace`    | 559 µs    |
| `low-hit-random`          | 1 µs      |
| `path-segment-fuzzy`      | 349 µs    |
| `high-hit-project`        | 14,600 µs |

估算常驻内存为 462,092,521 bytes（约 440.7 MiB），低于 500 MiB 目标；所有阈值断言通过。该数据验证查询结构，不代表 Windows 首次枚举耗时。

`index-source-benchmark` 在本机对仓库 `src` 目录的 smoke test 选择 Generic Scanner，10 个条目、1 个批次、0 个失败，并正确报告 `unsupportedPlatform` 的 MFT/USN 能力状态。

## 发布前 Windows 实机验收

下列北极星指标依赖真实 Windows、NTFS、Defender、磁盘和桌面事件循环，当前 macOS 环境不能给出有效结论：

- 约 200 万文件的首次快速可用时间和完整扫描是否不超过 90 秒；
- 托盘出现 P95、第二实例唤醒 P95；
- 三轮完整刷新时 SQLite 构建中、激活后、GC 后的真实磁盘体积；
- building、prepared、finalizing、activation 四个阶段的进程强杀恢复；
- 标准用户权限、无 UAC、盘符离线和 Defender 干扰下的 Win32/Generic 降级行为。

执行方法、环境字段和 JSON 采样格式见 `windows-ntfs-index-source.md`。这些项目是 Windows 发布验收门槛；在结果补录前不能宣称“NTFS 200 万文件 90 秒目标已达成”。
