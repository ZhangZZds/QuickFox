## ADDED Requirements

### Requirement: 全局唤醒组合键防误触

系统 SHALL 只在配置的修饰键和主键形成连续、有效的按键序列时触发全局唤醒。系统 MUST 防止修饰键状态在释放事件丢失、焦点切换或系统快捷键截获后无限期残留并误触发后续普通按键。

#### Scenario: 修饰键释放后普通 Space 不触发唤醒

- **WHEN** 用户配置全局唤醒键为 `Shift+Space`
- **AND** 系统收到 Shift 按下后又收到 Shift 释放
- **AND** 用户随后只按下 Space
- **THEN** QuickFox 不显示或隐藏启动窗口

#### Scenario: 修饰键状态超时后普通 Space 不触发唤醒

- **WHEN** 用户配置全局唤醒键为 `Shift+Space`
- **AND** 系统收到 Shift 按下
- **AND** 超过组合键有效窗口后用户按下 Space
- **THEN** QuickFox 不显示或隐藏启动窗口

#### Scenario: 打字序列中的 Space 不触发唤醒

- **WHEN** 用户配置全局唤醒键为 `Shift+Space`
- **AND** 系统收到 Shift 按下后又收到普通字符按下
- **AND** 用户随后按下 Space
- **THEN** QuickFox 不显示或隐藏启动窗口

#### Scenario: 有效组合键仍触发唤醒

- **WHEN** 用户配置全局唤醒键为 `Shift+Space`
- **AND** 系统在组合键有效窗口内连续收到 Shift 按下和 Space 按下
- **THEN** QuickFox 显示或隐藏启动窗口

#### Scenario: 同一次组合键不因重复 Space 多次唤醒

- **WHEN** 用户配置全局唤醒键为 `Shift+Space`
- **AND** 系统已因 Shift 按下后 Space 按下触发一次唤醒
- **AND** 用户未重新按下 Shift 但再次按下 Space
- **THEN** QuickFox 不再次显示或隐藏启动窗口

### Requirement: 全局唤醒键录制冲突反馈

系统 SHALL 在设置页录制全局唤醒键时识别已知高风险或系统保留组合键，并 MUST 阻止保存这些不可可靠捕获的组合键。

#### Scenario: Windows 系统保留 Alt+Space 录制失败并提示

- **WHEN** 用户在设置页录制全局唤醒键
- **AND** 用户尝试录制 `Alt+Space`
- **THEN** 设置页不保存该组合键
- **AND** 设置页显示该组合键可能被系统或当前窗口占用的错误提示

### Requirement: 全局唤醒桌面交互探索

系统 SHALL 为全局唤醒键维护可重复的自动化探索和手工验收清单，用于尽早发现真实桌面快捷键冲突、事件丢失和误触发问题。

#### Scenario: 状态机事件序列自动测试

- **WHEN** 开发者运行 Rust 单元测试
- **THEN** 测试覆盖组合键有效触发、释放后不触发、超时后不触发和重复主键不触发

#### Scenario: Windows 桌面验收覆盖系统保留组合键

- **WHEN** 发布前进行 Windows 手工验收
- **THEN** 验收清单包含 `Shift+Space` 打字不误触发和 `Alt+Space` 录制冲突提示
