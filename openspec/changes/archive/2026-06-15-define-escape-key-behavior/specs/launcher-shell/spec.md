## MODIFIED Requirements

### Requirement: 全局快捷键唤起启动器

系统 SHALL 使用用户配置的全局唤醒键切换 QuickFox 启动窗口的显示状态，并默认使用 `Shift+Shift`。当用户尚未配置或配置无效时，系统 MUST 回退到 `Shift+Shift`。

#### Scenario: 配置的唤醒键显示启动器

- **WHEN** QuickFox 启动窗口隐藏或位于后台
- **AND** 用户在任意应用中按下已配置的全局唤醒键
- **THEN** QuickFox 显示 Compact 启动窗口、置前窗口并聚焦搜索输入框

#### Scenario: 配置的唤醒键隐藏启动器

- **WHEN** QuickFox 启动窗口已显示且聚焦
- **AND** 用户再次按下已配置的全局唤醒键
- **THEN** QuickFox 隐藏启动窗口且不执行当前结果

#### Scenario: 默认双击 Shift 仍可唤醒

- **WHEN** 用户尚未配置全局唤醒键
- **AND** 用户连续按下两次 Shift
- **THEN** QuickFox 显示或隐藏 Compact 启动窗口

#### Scenario: Esc 关闭启动器

- **WHEN** QuickFox 启动窗口已显示
- **AND** 没有动作菜单、历史模式或其它局部状态打开
- **AND** 用户按下 Esc
- **THEN** QuickFox 关闭启动窗口且不执行当前结果

## ADDED Requirements

### Requirement: Esc 键逐层退出

系统 SHALL 在启动器和设置页中为 Esc 键提供逐层退出行为。当多个局部状态同时存在时，系统 MUST 先退出最局部的状态；只有普通启动器状态下才关闭启动器。

#### Scenario: 焦点离开输入框时 Esc 关闭启动器

- **WHEN** QuickFox 启动器已显示
- **AND** 焦点位于搜索输入框以外的启动器元素
- **AND** 没有动作菜单、历史模式或其它局部状态打开
- **THEN** 用户按下 Esc 时 QuickFox 关闭启动器
- **AND** 不执行当前结果
- **AND** 不记录输入历史

#### Scenario: 动作菜单打开时 Esc 只关闭菜单

- **WHEN** 搜索结果动作菜单已打开
- **AND** 用户按下 Esc
- **THEN** QuickFox 关闭动作菜单
- **AND** 保持启动器打开
- **AND** 不执行当前结果

#### Scenario: 历史模式打开时 Esc 只退出历史模式

- **WHEN** 输入历史模式已打开
- **AND** 用户按下 Esc
- **THEN** QuickFox 退出历史模式
- **AND** 保持启动器打开
- **AND** 保留当前搜索输入

#### Scenario: 命令预览中 Esc 关闭启动器

- **WHEN** 命令预览已显示
- **AND** 用户按下 Esc
- **THEN** QuickFox 关闭启动器
- **AND** 不执行命令
- **AND** 不记录输入历史

#### Scenario: 快捷键录制中 Esc 取消录制

- **WHEN** 设置页正在录制全局唤醒快捷键
- **AND** 用户按下 Esc
- **THEN** QuickFox 取消录制状态
- **AND** 保留原快捷键配置
- **AND** 保持设置页打开

#### Scenario: 设置页弹层打开时 Esc 关闭弹层

- **WHEN** 设置页轻量弹层已打开
- **AND** 用户按下 Esc
- **THEN** QuickFox 关闭该弹层
- **AND** 保持设置页打开

#### Scenario: 设置页常态 Esc 不关闭设置页

- **WHEN** 设置页没有快捷键录制或弹层状态
- **AND** 用户按下 Esc
- **THEN** QuickFox 保持设置页打开
