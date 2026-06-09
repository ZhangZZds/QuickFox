## ADDED Requirements

### Requirement: Windows 默认索引可用本地盘符

Windows 上系统 SHALL 在首次创建默认索引配置时发现可用本地盘符，并把可访问盘符根目录作为默认索引范围；如果没有发现可用盘符，系统 SHALL 回退到当前用户 profile 目录。

#### Scenario: 多个可用盘符进入默认索引范围

- **WHEN** Windows 机器存在 `C:\` 和 `D:\` 等可访问盘符
- **THEN** 默认索引范围包含这些可访问盘符根目录
- **AND** 不只包含当前用户 profile 目录

#### Scenario: 无可用盘符时回退用户目录

- **WHEN** Windows 盘符发现没有返回可访问目录
- **THEN** 系统使用当前用户 profile 目录作为默认索引范围

### Requirement: 索引扫描跳过系统噪音和构建缓存

系统 SHALL 默认跳过系统目录、回收站、隐藏噪音目录、构建产物、缓存目录和特殊系统文件；用户未显式排除的普通数据目录仍 SHALL 被索引。

#### Scenario: 隐式排除 Windows 系统噪音

- **WHEN** 系统构建默认扫描选项
- **THEN** 排除规则包含 `Windows`、`System Volume Information`、`$Recycle.Bin`、`AppData` 等系统噪音目录

#### Scenario: 隐式排除构建和缓存目录

- **WHEN** 系统构建默认扫描选项
- **THEN** 排除规则包含 `node_modules`、`target`、`.git`、`.cache`、`__pycache__`、`dist`、`build` 等构建和缓存目录

#### Scenario: 排除匹配不区分大小写

- **WHEN** 文件或目录名称大小写与排除规则不同
- **THEN** 扫描器仍跳过该文件或目录

### Requirement: 索引扫描减少无意义重复工作

系统 SHALL 在扫描前去重索引根目录，并在遇到单个目录项读取失败时记录失败并继续处理其他可用路径。

#### Scenario: 重复根目录只扫描一次

- **WHEN** 配置中出现重复索引根目录
- **THEN** 扫描器只索引该根目录一次

#### Scenario: 单个目录项失败不中断整轮扫描

- **WHEN** 扫描某个目录项时读取元数据失败
- **THEN** 系统记录该路径失败
- **AND** 继续扫描同级其他文件和目录
