## 1. SQLite baseline 安全

- [x] 1.1 增加未完成 baseline 不可见、重启清理的回归测试。
- [x] 1.2 增加 building/completed 状态并将完整 baseline 改为分块事务。
- [x] 1.3 启用 WAL、自动 checkpoint、journal size limit 和新库 incremental auto-vacuum。
- [x] 1.4 激活 baseline/保存 checkpoint 后分块清理失效完整批次。
- [x] 1.5 不再持久化可推导的 `search_text`，加载时重建。
- [x] 1.6 增加 8 GiB baseline 和 5 GiB 空闲空间安全边界。

## 2. Windows 启动安全

- [x] 2.1 Windows 首次默认范围改为现有用户热路径并增加测试。
- [x] 2.2 迁移与旧版自动生成值完全相同的全盘配置，保留显式自定义配置。
- [x] 2.3 加入 Tauri 单实例插件，重复启动唤起已有 launcher。

## 3. 文档与验证

- [x] 3.1 更新 OpenSpec delta、Windows QA 和数据库恢复说明。
- [x] 3.2 运行完整仓库检查和 `openspec validate prevent-index-storage-exhaustion --strict`。
      发布后验证边界：在 Windows `v1.6.1` 发布构建上记录单实例、数据库/WAL 峰值和低
      空间保护结果；该真实桌面验证不能由当前 macOS 自动化环境替代。
