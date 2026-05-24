## MODIFIED Requirements

### Requirement: 全局快捷键唤起启动器

系统 SHALL 注册 `Shift+Shift` 作为默认全局快捷键，并在用户连续按下两次
Shift 时切换 QuickFox 启动窗口的显示状态。

#### Scenario: 双击 Shift 显示启动器

- **WHEN** QuickFox 启动窗口隐藏或位于后台且用户在任意应用中连续按下两次 Shift
- **THEN** QuickFox 显示 Compact 启动窗口、置前窗口并聚焦搜索输入框

#### Scenario: 双击 Shift 隐藏启动器

- **WHEN** QuickFox 启动窗口已显示且用户连续按下两次 Shift
- **THEN** QuickFox 隐藏启动窗口且不执行当前结果

#### Scenario: Esc 关闭启动器

- **WHEN** QuickFox 启动窗口已显示且用户按下 Esc
- **THEN** QuickFox 关闭启动窗口且不执行当前结果

### Requirement: Compact 启动窗口

系统 SHALL 提供 Compact 风格启动窗口，包含搜索输入框和结果列表，不使用
营销页、多余说明或设置按钮作为第一屏内容。

#### Scenario: 启动窗口显示核心控件

- **WHEN** QuickFox 启动窗口打开
- **THEN** 窗口显示搜索输入框，并且不在快速启动窗口内显示设置按钮

#### Scenario: 空输入不显示结果区域留白

- **WHEN** 搜索输入框为空
- **THEN** QuickFox 不显示空白结果列表、底部留白或“未找到结果”区域

#### Scenario: 输入更新结果

- **WHEN** 用户在搜索输入框中输入查询文本
- **THEN** 结果列表根据当前查询刷新候选结果

### Requirement: 结果动作菜单

系统 SHALL 为支持次要动作的结果提供动作菜单，用户可以通过右键或键盘快捷
键打开该菜单，并且菜单位置应靠近触发的结果项。

#### Scenario: 右键打开动作菜单

- **WHEN** 用户右键点击一个文件或目录结果
- **THEN** 系统在该结果项附近显示其可用的次要动作列表

#### Scenario: 动作菜单执行次要动作

- **WHEN** 用户在动作菜单中选择“复制路径”
- **THEN** 系统复制该结果对应路径

## ADDED Requirements

### Requirement: 托盘菜单打开设置

系统 SHALL 通过系统托盘菜单提供设置入口，而不是在 Compact 启动窗口中显示
设置按钮。

#### Scenario: 托盘设置菜单打开设置页

- **WHEN** 用户在系统托盘菜单中选择“设置”
- **THEN** QuickFox 打开设置页或设置窗口

#### Scenario: 快速启动窗口不包含设置入口

- **WHEN** QuickFox Compact 启动窗口打开
- **THEN** 窗口只呈现搜索输入和搜索结果相关控件

### Requirement: 设置页分类管理

系统 SHALL 将设置按功能分类展示，便于后续扩展。

#### Scenario: 设置页显示功能分组

- **WHEN** 用户打开设置页
- **THEN** 设置页至少按“搜索与索引”“网页搜索”“历史”“命令执行”“外观与窗口”分组展示

#### Scenario: 搜索与索引分组提供刷新入口

- **WHEN** 用户打开设置页的“搜索与索引”分组
- **THEN** 系统显示手动刷新索引入口和当前索引配置

### Requirement: 搜索列表视觉约束

系统 SHALL 保持搜索结果列表紧凑，避免割裂的系统滚动条和不必要的空白区域。

#### Scenario: 结果溢出时列表内部滚动

- **WHEN** 搜索结果数量超过可见区域
- **THEN** 结果列表在启动窗口内部滚动且不显示割裂的页面级滚动条
