# QuickFox 1.6.0

## Highlights

- 文件创建、修改、删除和重命名现在会通过运行期 watcher 自动增量进入搜索结果；
  普通变化通常在 5 秒静默窗口后可见，并受首事件后 10 秒硬上限约束。
- 新增 targeted scanner、目录 manifest、分层 baseline/overlay/tombstone 搜索视图和
  SQLite delta journal，支持崩溃后的幂等恢复。
- name/path 增量不再等待正文索引，正文读取失败也不会撤销已经成功的文件名更新。
- 设置页帮助提示改为窗口级浮层，解决滚动容器裁切、窗口边缘遮挡和长文本不可读。

## Fixes

- 修复启动索引任务可能一直等待 Tauri `Ready` 事件，导致配置目录和新文件无法搜索。
- 自动迁移旧 `index_runtime_state` 和缺少 `payload_hash` 的 delta journal schema；保留
  当前可搜索 baseline，不需要删除配置或手工重建数据库。
- macOS watcher 重扫改为 targeted calibration，不再无条件触发完整 baseline refresh。
- watcher 在事件进入有界队列之前过滤排除路径，并正确处理 `/var` 与
  `/private/var` canonical 路径别名。
- 补强大索引候选召回、Unicode 路径匹配、内存计数与恢复错误传播。

## Upgrade notes

- 无需修改现有配置。首次启动 v1.6.0 时会自动执行 SQLite 兼容迁移。
- 自动增量索引默认开启，可在设置页关闭；关闭后仍保留最近可用搜索结果。
- 运行命令仍默认关闭；危险命令检查是防护栏，不是权限沙箱。

## Verification

- `npm run check`
  - 313 个前端测试通过
  - 520 个 Rust 测试通过，6 个显式 release/benchmark 测试按设计忽略
  - Prettier、ESLint、TypeScript、Vite build、rustfmt、Clippy 全部通过
- macOS 真实用户数据库空文件验证：generation 5 `upsert`、generation 6 `remove`，
  active baseline 保持不变
- 维护者确认 tooltip 与新增文件搜索的最终界面验收，并授权发布

## Validation scope

GitHub Actions 会分别构建 macOS 与 Windows 安装包。本次没有新增 Windows NTFS
多盘、休眠/唤醒及断盘恢复的结构化手工记录；维护者已明确接受该验证边界。完整变更
记录见仓库根目录 `CHANGELOG.md`。
