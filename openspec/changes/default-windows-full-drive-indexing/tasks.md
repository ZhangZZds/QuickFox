## 1. Windows 默认范围回归

- [x] 1.1 按 TDD 增加 Windows 首次配置默认包含全部可用盘符、无盘符时回退 profile 的失败测试
- [x] 1.2 按 TDD 增加 v1.6.1 自动热路径默认迁移到盘符范围、用户自定义配置不迁移的失败测试

## 2. 系统目录排除

- [x] 2.1 按 TDD 增加 Windows 系统目录/特殊文件默认排除且普通 `Users` 数据不排除的失败测试
- [x] 2.2 扩充统一隐式排除规则并验证 baseline、watcher 和 calibration 共用相同规则

## 3. 默认配置与迁移实现

- [x] 3.1 实现 Windows 默认盘符根目录选择和 profile fallback，保持其他平台不变
- [x] 3.2 将旧的全盘到热路径迁移替换为未修改 v1.6.1 热路径到全盘迁移，并保留用户自定义配置

## 4. 设置页与文档

- [x] 4.1 更新设置页帮助和盘符提示，说明 Windows 默认全盘、系统目录排除、后台补全和 partial/retry
- [x] 4.2 更新架构、故障排查、Windows 手工 QA 和存储恢复文档

## 5. 验证

- [x] 5.1 运行 `npm run check`、`openspec validate default-windows-full-drive-indexing --strict` 和 `git diff --check`
- [ ] 5.2 在真实 Windows 发布构建验证 C/D 盘默认范围、系统目录排除、离线盘 partial、资源占用和重启恢复
