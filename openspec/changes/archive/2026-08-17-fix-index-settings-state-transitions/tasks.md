## 1. 后端回归基线

- [x] 1.1 按 TDD 增加失败测试，证明索引语义配置在后台扫描前持久化、后台失败不回滚 desired config、重开设置读取新模式
- [x] 1.2 按 TDD 增加失败测试，证明 fast active roots 不包含配置大盘且 superseded refresh 可在 root 遍历中取消

## 2. 配置与 revision 状态流

- [x] 2.1 实现轻量 desired-config 提交：校验并持久化后递增 revision、保留旧搜索视图并安排后台 refresh
- [x] 2.2 暴露 applying/applied/partial/failed 配置应用状态，确保调度、watcher 或扫描失败不回滚已保存配置
- [x] 2.3 删除生产同步 config candidate/rollback 路径及只服务旧保存语义的死代码，保持现有 baseline identity fence

## 3. 范围、取消与部分失败

- [x] 3.1 让 baseline、standby/runtime watcher 和 calibration 共用按性能模式生成的 active roots，并修复 fast 热路径被 configured root 去重的问题
- [x] 3.2 为完整扫描增加 revision-aware 协作式取消并接入后台 worker，验证连续保存只发布最新 revision
- [x] 3.3 让不可用或失败 root 降级为 partial/dirty 状态，其他可用 roots 和最近可用索引继续工作

## 4. 设置页反馈与文案

- [x] 4.1 按 TDD 增加设置草稿、保存中、已保存、失败、索引应用中/失败以及关闭重开保持 balanced 的前端测试
- [x] 4.2 实现统一保存状态与结构化索引应用反馈，阻止重复提交并保留后台失败时的已保存字段值
- [x] 4.3 删除“保存后点击刷新生效”说明，将刷新定义为重试/校准，并为 Windows 盘符根目录显示风险提示

## 5. 文档与验证

- [x] 5.1 更新架构、故障排查和 Windows 手工 QA，记录 desired/applied revision、取消与大目录验收步骤
- [x] 5.2 运行格式化、ESLint、TypeScript/Vitest、rustfmt、Clippy、Rust 测试、`openspec validate fix-index-settings-state-transitions --strict` 和 `git diff --check`
- [ ] 5.3 在真实 Windows 发布构建记录 fast/balanced 切换、D 盘大目录、不可访问 root、连续保存、重开设置与重启恢复结果
