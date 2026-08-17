## Why

QuickFox 当前 Windows 首次配置只索引用户热路径，不符合默认覆盖本机全部文件名和路径的产品目标。Windows 默认范围需要覆盖所有当前可用盘符，同时主动跳过 C 盘系统目录和特殊文件，兼顾覆盖面、性能与稳定性。

## What Changes

- Windows 首次创建配置时，将当前可用盘符根目录作为默认 `include_dirs`，默认 `balanced` 仍先提供应用入口和用户热路径结果，再后台补全全盘范围。
- 扩充 Windows 隐式系统排除规则，跳过 `Windows`、`ProgramData`、`PerfLogs`、恢复/升级目录、回收站、卷元数据、用户 `AppData` 和虚拟内存文件等系统噪音。
- 将 v1.6.1 自动生成且未被用户修改的“用户热路径默认配置”迁移为全盘默认；用户自定义范围或任一索引字段后不迁移。
- 设置页和 Windows 文档说明默认全盘、系统目录排除、后台补全以及不可访问目录的 partial/retry 行为。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `actions-and-platform`: Windows 平台默认索引根目录从用户 profile 改为当前可用盘符根目录。
- `search-index`: Windows 首次配置和默认迁移改为全盘范围，并明确系统目录排除与用户自定义保护。
- `configuration-and-history`: 设置页需要说明 Windows 默认全盘范围与系统排除语义。

## Impact

- Rust 配置启动路径、Windows 盘符发现、默认配置迁移和隐式排除规则。
- Windows 首次索引的范围、耗时、状态反馈和存储占用。
- 设置页帮助文案、Windows 手工 QA、架构和故障排查文档。
- 不新增第三方 API；继续使用现有后台分阶段扫描、取消、partial root 和存储预算防护。
