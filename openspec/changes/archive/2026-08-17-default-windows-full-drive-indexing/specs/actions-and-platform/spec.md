## MODIFIED Requirements

### Requirement: 平台路径默认值

系统 SHALL 通过 PathAdapter 解析应用数据目录和默认索引根目录。

#### Scenario: Windows 默认索引路径

- **WHEN** 当前平台是 Windows 且没有自定义索引目录
- **THEN** PathAdapter 返回当前可用盘符根目录作为默认索引根目录
- **AND** 未发现可用盘符时回退当前用户 profile

#### Scenario: Linux 默认索引路径

- **WHEN** 当前平台是 Linux 且没有自定义索引目录
- **THEN** PathAdapter 返回用户 home 目录作为默认索引根目录
