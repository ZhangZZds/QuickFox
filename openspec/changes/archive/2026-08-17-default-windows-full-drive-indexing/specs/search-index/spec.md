## MODIFIED Requirements

### Requirement: 默认索引用户主目录

系统 SHALL 在 macOS/Linux 默认索引当前用户的主目录，在 Windows 默认索引当前可用盘符根目录，并只索引文件名、目录名和完整路径；系统 MUST 默认排除应用包内部、构建产物、缓存目录和系统隐藏噪音目录。

#### Scenario: 首次启动创建默认索引范围

- **WHEN** 用户首次启动 QuickFox 且没有自定义索引配置
- **THEN** macOS/Linux 使用当前用户主目录作为默认索引根目录
- **AND** Windows 使用当前可用盘符根目录作为默认索引根目录

#### Scenario: 索引不读取文件内容

- **WHEN** 系统刷新索引
- **THEN** 系统记录文件名、目录名和完整路径，但不读取文件内容

#### Scenario: macOS 应用包内部不进入普通索引

- **WHEN** 系统扫描 macOS `.app` 应用包
- **THEN** 系统将 `.app` 作为应用结果索引，并跳过 `.app/Contents` 内部文件

### Requirement: 索引扫描跳过系统噪音和构建缓存

系统 SHALL 默认跳过系统目录、回收站、隐藏噪音目录、构建产物、缓存目录和特殊系统文件；用户未显式排除的普通数据目录仍 SHALL 被索引。

#### Scenario: 隐式排除 Windows 系统噪音

- **WHEN** 系统构建 Windows 默认扫描选项
- **THEN** 排除规则包含 `Windows`、`ProgramData`、`PerfLogs`、`System Volume Information`、`$Recycle.Bin`、`Recovery`、`AppData`、Windows 升级目录和虚拟内存文件
- **AND** 排除规则不排除 `Users`、Desktop、Documents 等普通用户数据目录

#### Scenario: 隐式排除构建和缓存目录

- **WHEN** 系统构建默认扫描选项
- **THEN** 排除规则包含 `node_modules`、`target`、`.git`、`.cache`、`__pycache__`、`dist`、`build` 等构建和缓存目录

#### Scenario: 排除匹配不区分大小写

- **WHEN** 文件或目录名称大小写与排除规则不同
- **THEN** 扫描器仍跳过该文件或目录

## ADDED Requirements

### Requirement: Windows 默认索引全部可用盘符

Windows 上系统 SHALL 在首次创建默认索引配置时选择当前可用盘符根目录。默认 `balanced` 模式 MUST 先发布应用入口和用户热路径，再在后台补全盘符范围；盘符或目录失败 MUST 使用 partial/retry 语义，不得撤销默认配置。

#### Scenario: Windows 存在 C 盘和 D 盘

- **WHEN** 用户首次启动 QuickFox 且没有配置文件
- **THEN** 默认 `include_dirs` 包含 `C:\` 和 `D:\`
- **AND** 系统目录仍由隐式规则排除

#### Scenario: 未发现可用盘符

- **WHEN** Windows 盘符发现没有返回任何可用根目录
- **THEN** 默认索引范围回退当前用户 profile

#### Scenario: 现有配置仍是 v1.6.1 自动热路径默认

- **WHEN** 现有 Windows 索引配置与 v1.6.1 自动生成的用户热路径范围及其余默认索引字段完全一致
- **THEN** 系统将 `include_dirs` 迁移为当前可用盘符根目录
- **AND** 任一索引字段已被用户修改时不执行该迁移

#### Scenario: 单个默认盘符不可访问

- **WHEN** 默认全盘索引中一个盘符或子目录不可访问而其他盘符可用
- **THEN** 系统发布其他盘符的可用结果并报告 partial/dirty 状态
- **AND** 默认盘符配置保持不变并提供重试/校准入口
