# QuickFox 开发说明

## 目录结构

```text
src/                     前端 UI 与 Tauri client
src-tauri/src/core/      Rust core 模块
src-tauri/src/lib.rs     Tauri 集成层
openspec/                OpenSpec proposal / design / specs / tasks
docs/                    项目文档
```

## 常用命令

```bash
npm run dev
npm run tauri dev
npm run test
npm run rust:test
npm run check
openspec validate build-quickfox-launcher
npm run tauri build
```

## 开发流程

本项目默认遵循：

1. OpenSpec 明确 proposal / design / tasks
2. 按 TDD 先写失败测试
3. 写最小实现让测试转绿
4. 跑 `npm run check`
5. 跑 `openspec validate build-quickfox-launcher`

## 发布流程

本项目普通 CI 不产出安装包。正式发布使用 tag 触发：

```bash
npm run check
npm run tauri build
git tag v1.1.0
git push origin v1.1.0
```

`release.yml` 会使用 GitHub-hosted `macos-latest` 和 `windows-latest` runner
分别构建 Tauri bundle，并通过 `tauri-apps/tauri-action` 上传到 GitHub Release。

当前 release workflow 不做代码签名、公证或 Windows 证书签名；这些需要单独配置证书和密钥。

## 新增 Provider 的方法

1. 在 `src-tauri/src/core/providers.rs` 为新 Provider 写测试
2. 实现 `Provider` trait
3. 在运行时 `ProviderRegistry` 注册
4. 如有配置项，扩展 `QuickFoxConfig`
5. 更新架构与开发文档

网页搜索引擎不需要新增 Rust Provider。优先在设置页或 TOML 配置中添加前缀、名称和 URL 模板，模板必须包含 `{query}`。例如 DuckDuckGo：

```toml
[web_search.engines.ddg]
name = "DuckDuckGo"
url = "https://duckduckgo.com/?q={query}"
```

## 索引开发注意事项

- 文件索引不应在启动路径同步扫描大目录；启动时先加载 SQLite 中最近完成的索引快照
- `fast` / `balanced` / `complete` 必须产生可测试的扫描计划差异：`fast` 只扫应用入口和热路径，`balanced` 先快速可用再后台补全配置目录，`complete` 覆盖完整配置范围和可用盘符
- 阶段边界只在快速可用检查点和最终完成检查点写完整 SQLite 快照；中间补全阶段只更新内存索引和状态，避免反复写不断增长的聚合 batch
- 内容索引必须晚于基础 name/path 索引；`content:` 在内容索引准备中应返回明确反馈，不能伪装成 name/path 命中
- 后台刷新完成后用新批次替换内存索引，旧 generation 的刷新结果不能覆盖新请求
- 搜索路径不得为每次查询 clone 完整 `SearchIndex`；FileProvider 应借用或共享运行时索引，并保持大索引查询候选数有上限
- 文件 Provider 必须在索引不可用时降级为反馈，不影响计算器、网页搜索和命令 Provider
- 新增索引字段、状态或存储迁移时，需要同时补 Rust storage/index 测试和设置页状态测试

短期索引性能边界：

- QuickFox 当前不接入 Everything、Windows Search、NTFS USN Journal 或 MFT 读取；这些能力需要后续独立 OpenSpec 变更评估权限、平台差异和 fallback 语义
- 当前目标是让首次体验先快速可用，并让大目录补全可见、可控；不承诺 `C:\Users`、`D:\` 等大根目录瞬时完整索引
- 内容索引只处理配置范围内、大小限制内、可识别为文本的文件；PDF/Office 等专用 extractor 仍属于后续扩展

索引基准可用被忽略的 Rust 测试按需手动运行：

```bash
cargo test --manifest-path src-tauri/Cargo.toml indexing_baseline_fixture_reports_current_scanner_characteristics -- --ignored --nocapture
```

输出行 `QUICKFOX_INDEX_BASELINE` 包含 `scan_ms`、`entries`、`ordinary_query_us`、`search_index_clones` 和 `content_query_results`，用于记录快速阶段耗时、补全阶段耗时、阶段边界写入次数和大索引查询耗时的本机基线。真实 Windows 大目录仍以 `docs/windows-manual-qa.md` 的手工验收为准。

## 新增 Action 的方法

1. 在 `src-tauri/src/core/actions.rs` 扩展 `Action`
2. 增加序列化 / dispatcher 测试
3. 在 `execute_action` 中接入桌面行为
4. 如涉及平台差异，放入 `platform.rs`

## 新增平台 Adapter 的方法

1. 先在 `platform.rs` 写 mock 或命令构造测试
2. 把平台特有逻辑限制在 Adapter 内
3. 不要让 Provider 或前端直接依赖平台命令细节
4. 为 Windows / Linux / macOS 分支分别验证

## 桌面交互主动探索

全局快捷键、窗口焦点和系统菜单这类桌面交互不能只靠 macOS 日常使用推断。新增或修改相关逻辑时，按三层探索：

1. Rust core 先写事件序列表格测试，覆盖有效序列、释放事件丢失、超时、被普通输入打断、重复主键和配置切换。
2. 前端用 Testing Library 模拟 `keydown`，覆盖录制成功、Esc 取消、裸修饰键、已知系统保留组合键提示。
3. Windows/Linux 本机 QA 覆盖真实系统抢键和窗口行为；发现无法自动化的差异后，先记录到对应 manual QA，再尽量补回纯状态机或前端回归测试。

`Alt+Space` 这类组合通常会被 Windows 窗口系统菜单占用，jsdom 和 macOS CI 不能证明它在 Windows 可录制；需要在录制器中给出明确提示，并在 Windows 验收清单中保留检查项。

## 前端开发约定

- 不在 UI 中保留长期占位假数据
- 用户行为通过 `tauriClient.ts` 与后端通信
- 需要新增视图时优先补测试
- 保持启动器界面紧凑，不做营销页式布局

## 提交约定

- 以功能里程碑提交
- 每次提交前至少保证相关测试与检查通过
- 不在未验证状态下勾选 OpenSpec 任务
