## ADDED Requirements

### Requirement: macOS 与 Windows 增量索引发布验收

系统 SHALL 为影响运行期索引的发布维护 macOS 与 Windows 真实桌面验收记录，覆盖普通文件变化、事件风暴、休眠唤醒、失败降级和恢复路径。

#### Scenario: 普通文件变化验收

- **WHEN** 维护者在 macOS 或 Windows 发布构建的已监听目录中创建、修改、重命名和删除文件
- **THEN** 验收记录包含变化进入或离开搜索结果的耗时
- **AND** 普通变化在 watcher 到达后不超过 10 秒生效

#### Scenario: 事件风暴和休眠唤醒验收

- **WHEN** 维护者执行批量 Git checkout、依赖安装或大目录移动，或者让系统休眠后唤醒
- **THEN** 验收记录确认 QuickFox 窗口和查询保持响应
- **AND** watcher overflow 或重启状态可观察
- **AND** dirty-root 校准或后台刷新最终恢复一致结果

#### Scenario: watcher 失败降级验收

- **WHEN** 维护者模拟监听权限失败、root 断开或 watcher 初始化失败
- **THEN** 文件搜索继续使用最近可用 baseline
- **AND** 设置或状态反馈显示结构化失败摘要和可用恢复动作

#### Scenario: 发布前检查双平台记录

- **WHEN** 准备发布包含运行期索引变更的版本
- **THEN** 维护者检查最近一次 macOS 与 Windows 增量索引验收记录
- **AND** 记录缺失或门槛失败时不得声明该能力完成
