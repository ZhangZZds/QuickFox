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
```

## 开发流程

本项目默认遵循：

1. OpenSpec 明确 proposal / design / tasks
2. 按 TDD 先写失败测试
3. 写最小实现让测试转绿
4. 跑 `npm run check`
5. 跑 `openspec validate build-quickfox-launcher`

## 新增 Provider 的方法

1. 在 `src-tauri/src/core/providers.rs` 为新 Provider 写测试
2. 实现 `Provider` trait
3. 在运行时 `ProviderRegistry` 注册
4. 如有配置项，扩展 `QuickFoxConfig`
5. 更新架构与开发文档

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
