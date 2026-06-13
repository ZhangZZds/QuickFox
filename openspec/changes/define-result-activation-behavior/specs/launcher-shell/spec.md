## ADDED Requirements

### Requirement: 搜索结果主动作激活

系统 SHALL 为搜索结果定义统一的主动作激活语义。用户通过鼠标左键单击某个结果，或通过 Enter 激活当前选中结果时，系统 MUST 执行该结果由 Rust core 返回的主动作。

#### Scenario: 左键单击文件夹结果打开文件夹

- **WHEN** 用户左键单击一个目录搜索结果
- **THEN** QuickFox 执行该目录结果的主动作
- **AND** 系统通过当前平台默认方式打开该目录

#### Scenario: 左键单击文件结果使用默认应用打开

- **WHEN** 用户左键单击一个文件搜索结果
- **THEN** QuickFox 执行该文件结果的主动作
- **AND** 系统通过当前平台默认关联应用打开该文件

#### Scenario: 左键单击应用结果启动应用

- **WHEN** 用户左键单击一个应用搜索结果
- **THEN** QuickFox 执行该应用结果的主动作
- **AND** 系统通过当前平台默认方式启动该应用

#### Scenario: Enter 激活当前选中结果

- **WHEN** 搜索结果列表中存在当前选中结果
- **AND** 用户按下 Enter
- **THEN** QuickFox 执行当前选中结果的主动作
- **AND** 该动作 SHALL 与左键单击同一结果时执行的动作一致

#### Scenario: 右键菜单仍只执行次要动作

- **WHEN** 用户右键点击搜索结果
- **THEN** QuickFox 显示该结果的次要动作菜单
- **AND** 不因右键打开菜单而执行该结果的主动作
