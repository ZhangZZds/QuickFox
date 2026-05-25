# QuickFox 架构说明

## 总览

QuickFox 采用 Tauri 双端架构：

- 前端负责窗口 UI、输入交互、结果渲染和设置表单
- Rust core 负责查询解析、索引、Provider、Action、配置、历史和平台适配

## 模块边界

### 前端

- `src/App.tsx`
  - 启动器主界面
  - 搜索输入、结果列表、命令预览、设置页
- `src/tauriClient.ts`
  - 前端与 Tauri commands 的边界

### Rust core

- `src-tauri/src/core/search.rs`
  - 查询解析
  - 结果模型
  - 排序器
- `src-tauri/src/core/index.rs`
  - 文件/目录索引扫描
  - 索引状态模型
  - 模糊搜索与正则搜索
- `src-tauri/src/core/providers.rs`
  - FileProvider
  - CalculatorProvider
  - WebSearchProvider
  - CommandProvider
- `src-tauri/src/core/actions.rs`
  - 统一 Action 模型
  - Action dispatcher
- `src-tauri/src/core/platform.rs`
  - 路径适配
  - 打开行为抽象
  - 终端命令构造
  - 命令安全检查
- `src-tauri/src/core/config.rs`
  - TOML 配置模型与默认配置
- `src-tauri/src/core/storage.rs`
  - SQLite 持久化、历史与索引快照

### Tauri 集成层

- `src-tauri/src/lib.rs`
  - 应用运行时状态
  - Tauri commands 暴露
  - 菜单栏图标与窗口显示逻辑
  - 桌面动作执行

## 数据流

```text
前端输入
  -> tauriClient.search()
  -> Rust QueryParser
  -> ProviderRegistry
  -> SearchIndex / Calculator / Web / Command
  -> Ranker
  -> SearchResult[]
  -> 前端列表渲染
  -> 用户触发动作
  -> tauriClient.executeAction()
  -> Rust execute_action
  -> 平台打开 / 剪贴板 / 外部终端
```

## Provider / Action / Adapter 关系

- Provider 只负责“产出结果”，不直接执行系统行为
- SearchResult 只携带标准化 Action
- Action 执行统一收口到 Rust
- 平台差异隔离在 Adapter 或平台命令构造中

## 运行时状态

Tauri 启动时会初始化一份运行时状态，包含：

- 当前配置
- 当前内存索引
- 最近一次索引报告
- 当前索引状态

启动时优先从 SQLite 读取最近完成的索引快照并构造内存索引；没有快照时文件 Provider 会降级为状态反馈。实际文件扫描在后台线程中运行，完成后写入新的 SQLite 索引批次，再用 generation 检查原子替换内存索引，避免旧刷新任务覆盖新结果。

搜索和刷新索引都基于这份状态工作，而不是每次由前端硬编码构造假数据。计算器、网页搜索和命令 Provider 不依赖文件索引，因此索引构建期间仍可用。

## 当前已知限制

- 设置页已支持索引规则、网页搜索引擎、历史和命令安全等核心配置；更细的外观配置仍较轻量
- 命令执行走外部终端，强确认 UI 还没有单独展开
- 历史持久化与排序权重已经有 core 边界，但前端还未完整可视化
