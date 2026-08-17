# Windows 默认全盘索引设计

## 目标

Windows 首次配置默认把当前可用盘符写入 `include_dirs`，默认 `balanced` 仍先发布应用入口和用户热路径，然后后台补全盘符。C 盘系统目录、恢复/升级目录、卷元数据、回收站、用户缓存和特殊系统文件通过统一隐式规则跳过。

## 关键行为

- 新安装：默认 `include_dirs = ["C:\\", "D:\\", ...]`。
- 无可用盘符：回退当前用户 profile。
- v1.6.1 未修改的自动热路径默认：迁移为盘符范围。
- 自定义 include/exclude、模式、性能、内容或 watcher 字段：不迁移。
- `Users`、Desktop、Documents 不进入系统强制排除。
- 单盘或子目录失败：其他盘符继续应用，配置保持，状态为 partial 并可重试。

## 性能与安全

默认模式不改为 `complete`，避免阻塞快速可用阶段。全盘补全继续受 revision 取消、8 GiB baseline、5 GiB 剩余空间和 partial root 防护约束。默认只索引 name/path，正文范围不扩大。
