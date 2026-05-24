# launcher-shell Specification

## Purpose
TBD - created by archiving change build-quickfox-launcher. Update Purpose after archive.
## Requirements
### Requirement: 全局快捷键唤起启动器

系统 SHALL 注册 `Shift+Shift` 作为默认全局快捷键，并在用户连续按下两次
Shift 时显示 QuickFox 启动窗口。

#### Scenario: 双击 Shift 显示启动器

- **WHEN** 用户在任意应用中连续按下两次 Shift
- **THEN** QuickFox 显示 Compact 启动窗口并聚焦搜索输入框

#### Scenario: Esc 关闭启动器

- **WHEN** QuickFox 启动窗口已显示且用户按下 Esc
- **THEN** QuickFox 关闭启动窗口且不执行当前结果

### Requirement: Compact 启动窗口

系统 SHALL 提供 Compact 风格启动窗口，包含搜索输入框和结果列表，不使用
营销页或多余说明作为第一屏。

#### Scenario: 启动窗口显示核心控件

- **WHEN** QuickFox 启动窗口打开
- **THEN** 窗口显示搜索输入框和结果列表区域

#### Scenario: 输入更新结果

- **WHEN** 用户在搜索输入框中输入查询文本
- **THEN** 结果列表根据当前查询刷新候选结果

### Requirement: 键盘优先结果导航

系统 SHALL 支持用键盘在结果列表中导航，并用 Enter 执行当前选中结果的主
动作。

#### Scenario: 上下方向键移动选择

- **WHEN** 结果列表包含多个结果且用户按下上/下方向键
- **THEN** 当前选中结果按对应方向移动

#### Scenario: Enter 执行主动作

- **WHEN** 结果列表中存在选中结果且用户按下 Enter
- **THEN** 系统执行该结果的主动作

### Requirement: 结果动作菜单

系统 SHALL 为支持次要动作的结果提供动作菜单，用户可以通过右键或键盘快捷
键打开该菜单。

#### Scenario: 右键打开动作菜单

- **WHEN** 用户右键点击一个文件或目录结果
- **THEN** 系统显示该结果可用的次要动作列表

#### Scenario: 动作菜单执行次要动作

- **WHEN** 用户在动作菜单中选择“复制路径”
- **THEN** 系统复制该结果对应路径

### Requirement: 命令模式预览

系统 SHALL 在命令查询触发时将结果区域切换为 preview/确认样式，而不是普通
文件结果列表。

#### Scenario: 命令查询显示确认视图

- **WHEN** 命令执行已启用且用户输入命令前缀和命令文本
- **THEN** 结果区域显示待执行命令、目标终端和确认动作

