## Context

v1.6.1 为避免 Windows 首次启动扫描 C/D 盘，将默认 `include_dirs` 改为 Desktop、Documents、Downloads、Projects 和 workspace。现在产品目标调整为默认覆盖当前可用盘符，但上一变更已经把配置保存和索引应用解耦，因此全盘扫描可以继续在后台运行，单个盘符或目录失败也只产生 partial 状态。

当前扫描器已有一组隐式名称排除规则，但缺少 `ProgramData`、`PerfLogs` 和 Windows 升级目录。启动配置还包含一个把旧全盘默认迁移到热路径的单向迁移，需要反转为仅识别 v1.6.1 自动热路径默认值。

## Goals / Non-Goals

**Goals:**

- Windows 首次配置默认包含当前可用的 `C:\` 到 `Z:\` 盘符根目录。
- 保持 `balanced` 为默认模式，应用入口和用户热路径先可用，全盘范围后台补全。
- 默认跳过 C 盘典型系统目录、恢复/升级目录、卷元数据、回收站、用户缓存和特殊系统文件。
- 只迁移未修改的 v1.6.1 自动热路径默认配置，不覆盖用户自定义索引设置。
- 保持 macOS/Linux 默认范围不变。

**Non-Goals:**

- 不承诺全盘首次索引瞬时完成。
- 不读取 NTFS MFT/USN Journal，也不接入 Everything 或 Windows Search。
- 不默认读取文件正文；内容索引仍使用单独的有限目录配置。
- 不绕过现有 8 GiB baseline 和 5 GiB 剩余空间保护。

## Decisions

### Windows 默认根目录使用当前可用盘符

`default_index_dirs()` 在 Windows 上优先复用盘符发现结果；至少发现一个盘符时直接写入盘符根目录，未发现时才回退用户 profile。这样配置文件明确表达默认全盘范围，现有 `balanced` 扫描计划会先完成应用入口和热路径，再处理 configured-roots。

备选方案是把性能模式默认改为 `complete`。该方案会扩大阶段和剩余盘符语义，并降低首次可用速度，因此保留 `balanced`。

### 系统排除集中在隐式规则

在现有 `implicit_exclude_patterns()` 中补充 `ProgramData`、`PerfLogs`、`Documents and Settings`、`$WinREAgent`、`$Windows.~BT`、`$Windows.~WS`、`Config.Msi` 和 `MSOCache`。这些规则与已有 `Windows`、`Recovery`、`AppData`、回收站、卷元数据及虚拟内存文件一起应用于全盘 baseline、watcher 和 calibration。

不默认排除 `Users`，确保 Desktop、Documents 和普通用户数据可被索引；不排除 `Program Files`，因为 QuickFox 仍需要应用发现，且 Windows application stage 会独立扫描应用入口。

### 迁移只识别完整默认指纹

启动时计算 v1.6.1 的热路径默认集合和当前盘符集合。仅当现有 `IndexConfig` 与“热路径集合 + 其余默认索引字段”完全一致时，把 `include_dirs` 改为盘符集合。现有全盘配置保持不变；任一 exclude、模式、性能模式、内容目录、ignore 或 watcher 字段被修改时不迁移。

### 状态与恢复沿用 desired/applied revision

默认全盘不新增保存事务。盘符暂时不可访问时，现有 partial/dirty 状态保留其他盘符结果；恢复后 root monitor 或“重试/校准索引”继续应用。连续修改仍通过 revision-aware 取消停止旧扫描。

## Risks / Trade-offs

- [首次索引 CPU、I/O、SQLite 占用明显增加] → 保持 `balanced` 分阶段、后台 worker、取消和存储预算保护，并默认排除高噪音目录。
- [盘符存在但临时离线或读取失败] → 使用 partial root 语义，不阻止配置持久化或其他盘符发布。
- [用户恰好手工配置成 v1.6.1 默认热路径且未改其他字段] → 可能被识别为自动默认；迁移条件限定完整索引配置指纹，并在文档中说明可重新保存自定义范围。
- [按名称排除可能跳过其他盘同名普通目录] → 系统目录名称保持窄集合；用户可通过更具体的自定义范围规避，但强制系统排除不提供关闭入口。

## Migration Plan

1. 新安装在 Windows 创建盘符根目录默认配置。
2. 启动现有配置时，只把未修改的 v1.6.1 热路径默认迁移为当前盘符集合并持久化一次。
3. 用户自定义配置和其他平台配置不变。
4. 如需回退版本，配置中的盘符根目录仍可被旧版本读取；旧版本可能再次执行其安全默认迁移，因此回退前应备份配置。

## Open Questions

- 真实 Windows 发布构建需要记录系统盘、数据盘、离线盘和大目录的首次索引耗时与资源占用，后续再决定是否引入 NTFS 专用 Provider。
