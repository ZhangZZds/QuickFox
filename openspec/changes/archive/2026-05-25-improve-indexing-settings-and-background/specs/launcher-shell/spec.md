## ADDED Requirements

### Requirement: 索引状态提示

系统 SHALL 在启动器或设置页中提示文件索引状态，避免用户误以为整个 QuickFox 不可用。

#### Scenario: 文件搜索不可用提示

- **WHEN** 用户输入普通文件查询且没有可用文件索引
- **THEN** 启动器显示文件搜索正在准备或不可用的反馈，同时保留其他 Provider 结果

#### Scenario: 使用旧索引提示

- **WHEN** 后台索引刷新中且存在旧索引快照
- **THEN** 启动器或设置页提示正在更新索引，并允许文件搜索使用旧快照

### Requirement: 设置页分区控制台

系统 SHALL 提供更完整的设置页，按索引、网页搜索、历史、命令安全和外观/窗口分区展示配置。

#### Scenario: 打开设置页看到分区导航

- **WHEN** 用户从托盘菜单打开设置页
- **THEN** 设置页显示清晰的分区导航和当前分区内容

#### Scenario: 新增网页搜索使用向导弹层

- **WHEN** 用户在网页搜索分区选择新增搜索引擎
- **THEN** 系统显示轻量向导式弹层，引导填写前缀、名称和 URL 模板

## MODIFIED Requirements

### Requirement: 全局快捷键唤起启动器

系统 SHALL 注册 `Shift+Shift` 作为默认全局快捷键，并在用户连续按下两次 Shift 时显示 QuickFox 启动窗口；后台索引不得阻塞该唤醒行为。

#### Scenario: 双击 Shift 显示启动器

- **WHEN** 用户在任意应用中连续按下两次 Shift
- **THEN** QuickFox 显示 Compact 启动窗口并聚焦搜索输入框

#### Scenario: Esc 关闭启动器

- **WHEN** QuickFox 启动窗口已显示且用户按下 Esc
- **THEN** QuickFox 关闭启动窗口且不执行当前结果

#### Scenario: 索引中仍可唤醒

- **WHEN** 文件索引正在后台构建
- **THEN** 用户双击 Shift 仍能显示或隐藏 QuickFox 启动窗口
