# Index Settings State Transition 设计

## 目标

让设置保存表达用户期望，而不是以全盘索引成功作为提交条件。`config.toml` 保存后，后台 index revision 可应用、失败、部分完成或被更新 revision 取代；任何状态都不能静默把用户选择从 `balanced` 回滚为 `fast`。

## 核心不变量

1. 配置校验和持久化是保存成功的唯一前置条件；文件系统遍历、watcher、内容索引和 baseline 写入不在保存事务中。
2. desired revision 单调递增；worker、持久化和发布只有 identity/fingerprint 仍匹配最新 revision 时才能更新 runtime。
3. 新 baseline 成功发布前，最近可用 layered search view 始终可搜索；后台失败不清空旧 view，也不恢复旧配置。
4. `fast`、`balanced`、`complete` 产生唯一 active-root 集合，baseline、standby/runtime watcher 和 calibration 不得各自扩大范围。
5. superseded scan 在 root/entry 边界协作式取消；旧结果不得覆盖新状态。
6. 单 root 失败是 partial/dirty 状态；其他 roots 继续，只有没有任何可用范围或持久化一致性失败才整体失败。
7. 设置页分别显示草稿保存与 index revision 应用状态，重新打开始终从 desired config 恢复控件值。

## 模块边界

- `core/config.rs` 继续负责配置分类和校验。
- scanner 提供 cancellable 全量遍历边界，不依赖 Tauri。
- `lib.rs` 负责 desired revision 提交、worker 调度、identity fence 和状态事件接线。
- React 只维护草稿/请求 UI 并展示 Rust index status，不自行推断后台完成。

## 测试策略

- 每个行为先增加失败测试并单独运行确认 RED。
- Rust 测试覆盖持久化顺序、失败不回滚、active roots、entry 级取消、stale publish 和 partial root。
- 前端测试覆盖按钮和 aria-live 状态、重复提交、错误反馈、重新加载 desired config 与 D 盘风险提示。
- 完成前运行完整本地检查；真实 Windows 行为记录在手工 QA，未执行不得宣称完成 Windows 验收。
