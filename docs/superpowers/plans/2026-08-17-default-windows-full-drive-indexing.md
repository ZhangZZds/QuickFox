# Default Windows Full Drive Indexing Implementation Plan

**Goal:** Windows 默认覆盖当前可用盘符，同时跳过系统目录并安全迁移 v1.6.1 自动热路径默认。

**Architecture:** 盘符根目录写入默认 `include_dirs`，`balanced` 扫描计划负责快速/后台分阶段；隐式排除规则统一作用于 baseline、watcher 和 calibration；迁移只匹配完整默认索引配置。

**Tech Stack:** Rust、Tauri、React/TypeScript、Vitest、OpenSpec。

### Task 1: 默认盘符与 fallback

- [x] 写失败测试：C/E 可用时默认返回 C/E，无可用盘符时回退 profile。
- [x] 实现可注入的 Windows 默认目录选择函数并接入启动路径。
- [x] 运行定向测试确认 GREEN。

### Task 2: 默认迁移

- [x] 写失败测试：未修改热路径默认迁移到盘符，任一索引字段修改后不迁移。
- [x] 替换旧迁移方向并保持现有全盘配置不变。
- [x] 运行定向测试确认 GREEN。

### Task 3: 系统排除

- [x] 写失败测试：新增 Windows 系统目录和特殊文件被排除，`Users` 不被排除。
- [x] 扩充隐式排除规则。
- [x] 运行 scanner/scan-options 定向测试确认 GREEN。

### Task 4: UI、文档与验证

- [x] 更新 Windows 默认全盘帮助文案和前端断言。
- [x] 更新架构、故障排查、Windows QA 和存储恢复文档。
- [x] 运行完整 `npm run check`、OpenSpec strict validate 和 `git diff --check`。
- [ ] 真实 Windows 发布构建验收保留为未完成项，直到有 Windows 环境结果。
