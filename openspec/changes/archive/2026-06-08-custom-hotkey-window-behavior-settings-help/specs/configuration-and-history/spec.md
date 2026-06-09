## ADDED Requirements

### Requirement: 全局唤醒键配置持久化

系统 SHALL 在 QuickFox 配置中持久化全局唤醒键，并在启动、保存和运行时使用该配置。

#### Scenario: 默认配置包含双击 Shift

- **WHEN** QuickFox 首次创建默认配置
- **THEN** 配置包含全局唤醒键默认值 `Shift+Shift`

#### Scenario: 保存自定义唤醒键

- **WHEN** 用户在设置页录制 `Control+Space` 并保存设置
- **THEN** 配置文件持久化该全局唤醒键
- **AND** 后续启动继续使用该唤醒键

#### Scenario: 无效唤醒键回退默认值

- **WHEN** 配置文件包含无法解析的全局唤醒键
- **THEN** 系统使用 `Shift+Shift` 作为运行时回退
- **AND** 设置页显示可操作的状态提示

#### Scenario: 保存配置校验唤醒键

- **WHEN** 用户保存空唤醒键或单个非双击修饰键
- **THEN** 系统拒绝保存或回退为有效默认值
- **AND** 设置页提示用户录制组合键或双击 Shift
