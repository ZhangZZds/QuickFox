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
- 后台刷新完成后用新批次替换内存索引，旧 generation 的刷新结果不能覆盖新请求
- 文件 Provider 必须在索引不可用时降级为反馈，不影响计算器、网页搜索和命令 Provider
- 新增索引字段、状态或存储迁移时，需要同时补 Rust storage/index 测试和设置页状态测试

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

## 前端开发约定

- 不在 UI 中保留长期占位假数据
- 用户行为通过 `tauriClient.ts` 与后端通信
- 需要新增视图时优先补测试
- 保持启动器界面紧凑，不做营销页式布局

## 提交约定

- 以功能里程碑提交
- 每次提交前至少保证相关测试与检查通过
- 不在未验证状态下勾选 OpenSpec 任务
