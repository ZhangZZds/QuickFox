# Windows NTFS 索引源与性能验收

本文记录 QuickFox 1.7 的 `IndexSource` 边界、Windows NTFS 能力探测、无服务降级路径和真实机器验收方法。

## 已实现策略

- `IndexSource` 统一输出流式 `IndexedEntry` 批次、累计扫描统计、阶段进度和逐目录 checkpoint；调用方可传回未完成目录前沿与已完成统计继续扫描。取消以明确错误返回，目录中途取消不会提交该目录完成，调用方不能把部分结果标记为完整根。
- `GenericIndexSource` 包装现有 ignore-aware Scanner，保留跨平台行为。
- `WindowsNtfsIndexSource` 对每个根探测卷类型和文件系统。固定 NTFS 卷且未启用项目 ignore 规则时，使用 `FindFirstFileExW(FindExInfoBasic, FIND_FIRST_EX_LARGE_FETCH)` 批量枚举；网络盘、可移动盘、非 NTFS、探测异常和需要项目 ignore 语义时降级到 Generic Scanner。
- 扫描前按 Windows 大小写不敏感、`\`/`/` 等价和路径段边界压缩覆盖根。父卷已覆盖 Desktop、Downloads 等热点目录时，同一批计划不会再次完整扫描子根。
- Win32 枚举跳过目录 reparse point 的递归，避免 junction/symlink 环；排除规则继续由 `IndexPathRules` 统一执行。
- MFT/USN 探测只以共享、零期望访问权限打开卷并查询 Journal。代码不会请求 UAC、安装服务或静默提权。

## MFT/USN 决策

当前版本不把原始 MFT/USN 枚举设为默认路径。标准用户能否打开卷和查询 Journal 受 Windows 版本、卷策略与企业安全配置影响；即使 Journal 可读，稳定的 MFT 路径重建、删除记录处理和安全更新仍需要单独设计与 Windows 实机验证。

能力探测会区分：非 Windows、非 NTFS、无法打开原始卷、Journal 查询失败，以及“Journal 可读但原始枚举未启用”。后四种情况都不会触发提权；固定 NTFS 采用无服务 Win32 批量枚举，其余采用 Generic Scanner。

如果后续引入高权限服务，必须先单独评审：服务账户和最小权限、IPC 身份认证、允许访问的卷/操作、安装与卸载、签名和升级、日志脱敏、故障降级。评审通过前不得把能力探测结果用作自动提权依据。

## 真实 Windows 基准工具

使用 release 构建，参数必须是待测卷根或代表性大目录：

```powershell
cd src-tauri
cargo run --release --bin index-source-benchmark -- D:\
```

工具输出 JSON，包括能力探测结果、最终选择的来源、耗时、条目数、批次数、失败数和最后进度。测试前记录：

- Windows 版本与补丁、CPU、内存；
- 磁盘型号、SSD/HDD、文件系统、卷容量和可用空间；
- 文件与目录总数、杀毒软件实时扫描状态；
- 进程是否为标准用户；不得为了得到更好结果改用管理员运行；
- 冷缓存/热缓存条件，各运行至少三次并报告中位数和最差值。

目标数据集为约 200 万文件，完整扫描目标不超过 90 秒。还需在 QuickFox 应用内测量首个热点目录可搜索时间，不能用 CLI 完整扫描耗时代替“快速可用”指标。

## 本次自动化结果与待验收项（2026-08-18）

开发环境为 macOS，无法代表 NTFS、Win32 枚举、USN Journal 权限或 Windows Defender 干扰。本次只在自动化测试中验证了：

- Windows 路径大小写、分隔符、device/UNC 前缀规范化；
- 父根覆盖压缩与相邻前缀路径不误合并；
- Generic 流式批次、阶段进度和显式取消；
- Generic 对现有可续扫 Scanner 的逐目录 checkpoint 转发；
- 非 Windows/项目 ignore 场景安全降级。

真实 Windows NTFS 200 万文件基准尚未执行，因此不得声称“90 秒目标已达成”。发布前必须把三次原始 JSON、环境信息和应用内快速可用时间补到本节或独立的 Windows QA 结果文档，并在标准用户权限下人工验证：能力说明准确、无 UAC 弹窗、取消后根不会被标记为完成、不可读子目录只导致该路径降级。
