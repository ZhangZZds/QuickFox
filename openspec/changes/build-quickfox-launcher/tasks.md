## 1. 项目脚手架与质量门

- [x] 1.1 初始化 Tauri + React + Vite + TypeScript 项目结构，并删除与 QuickFox 无关的生成示例代码
- [x] 1.2 建立 Rust 后端模块目录，避免核心逻辑堆在 `main.rs`
- [x] 1.3 配置 TypeScript、ESLint、Prettier、Vitest 和前端测试脚本
- [x] 1.4 配置 Rust `rustfmt`、`clippy`、测试命令和必要依赖
- [x] 1.5 增加统一本地检查命令，覆盖 Rust 与前端格式、lint、测试和构建
- [x] 1.6 补充 `.gitignore`，排除构建产物、本地数据库、日志和 `.superpowers/`

## 2. Rust Core 基础模型

- [x] 2.1 先为 `SearchResult`、`Action`、Provider trait 和 Action dispatcher 写单元测试
- [x] 2.2 实现统一 `SearchResult`、`Action`、Provider trait 和 Provider registry
- [x] 2.3 先为查询解析、前缀识别和模式判断写单元测试
- [x] 2.4 实现查询解析器，支持普通查询、正则前缀、网页搜索前缀和命令前缀
- [x] 2.5 先为 `Ranker` 排序规则写单元测试
- [x] 2.6 实现 `Ranker` 边界，结合模糊匹配、精确子串、路径深度和历史权重

## 3. 配置、存储与历史

- [x] 3.1 先为默认 TOML 配置创建和配置加载写单元测试
- [x] 3.2 实现 TOML 配置结构、默认配置生成和配置文件读写
- [x] 3.3 先为无效配置场景写单元测试，包括缺少 `{query}` 的网页搜索模板
- [x] 3.4 实现配置校验和安全回退错误
- [x] 3.5 先为 SQLite storage layer 的迁移、读写和测试隔离写测试
- [x] 3.6 实现 SQLite storage layer，用于索引状态和历史持久化
- [x] 3.7 先为文件/目录历史和最近 15 条命令历史写单元测试
- [x] 3.8 实现文件/目录使用历史、命令历史持久化、清空、关闭和最大条数设置

## 4. 索引与搜索

- [x] 4.1 先为默认索引根目录解析写平台分支测试
- [x] 4.2 实现 PathAdapter 的 Windows/Linux/macOS 默认路径解析
- [x] 4.3 先为索引扫描、包含目录、排除目录和排除模式写单元测试
- [x] 4.4 实现文件/目录名称与路径索引扫描，不读取文件内容
- [x] 4.5 先为手动刷新和部分目录失败继续处理写测试
- [x] 4.6 实现手动刷新索引和失败目录报告
- [x] 4.7 先为普通模糊搜索、显式正则搜索和无效正则反馈写测试
- [x] 4.8 实现模糊搜索、`re:` 默认正则搜索和可配置正则前缀

## 5. 内置 Providers

- [x] 5.1 先为 FileProvider 返回文件/目录结果和主动作写测试
- [x] 5.2 实现 FileProvider 并接入 Provider registry
- [x] 5.3 先为计算器表达式写测试，覆盖 `2^10`、`sqrt(9)`、`0xff` 和百分比
- [x] 5.4 实现 CalculatorProvider 和复制结果动作
- [x] 5.5 先为网页搜索前缀、URL 编码和显式触发规则写测试
- [x] 5.6 实现 WebSearchProvider，确保无本地结果时不自动网页搜索
- [x] 5.7 先为命令执行开关、命令前缀和命令结果确认状态写测试
- [x] 5.8 实现 CommandProvider，未启用时返回启用提示，启用时返回确认型命令结果

## 6. Actions 与平台 Adapter

- [x] 6.1 先为 Action dispatcher 路由到 Adapter 写单元测试
- [x] 6.2 实现 Action dispatcher，集中处理打开、复制、网页 URL 和命令执行动作
- [x] 6.3 先为 OpenAdapter 的文件、目录、所在目录和 URL 行为写 mock 测试
- [x] 6.4 实现 OpenAdapter 平台抽象
- [x] 6.5 先为 Windows Terminal `wt.exe` 命令构造写测试
- [x] 6.6 实现 Windows TerminalAdapter
- [x] 6.7 先为 Linux 终端 fallback 顺序写测试
- [x] 6.8 实现 Linux TerminalAdapter fallback
- [x] 6.9 先为危险命令 deny/confirm 规则写测试
- [x] 6.10 实现命令执行安全检查、普通确认和危险命令强确认/阻止

## 7. 前端启动器 UI

- [ ] 7.1 先为搜索输入、结果列表渲染和选中状态写 UI 测试
- [ ] 7.2 实现 Compact 启动窗口基础布局
- [ ] 7.3 先为上下方向键、Enter 和 Esc 行为写 UI 测试
- [ ] 7.4 实现键盘导航、主动作执行和 Esc 关闭
- [ ] 7.5 先为右键/快捷键动作菜单写 UI 测试
- [ ] 7.6 实现结果动作菜单，支持打开、打开所在目录和复制路径
- [ ] 7.7 先为命令模式 preview/确认视图写 UI 测试
- [ ] 7.8 实现命令模式 preview/确认 UI
- [ ] 7.9 实现基础设置页，覆盖索引目录、前缀、命令执行开关和历史设置

## 8. Tauri 集成

- [ ] 8.1 先为前端调用 Rust 命令的契约写 mock 测试
- [ ] 8.2 暴露搜索、执行动作、刷新索引、读取/保存配置和历史操作的 Tauri commands
- [ ] 8.3 集成全局快捷键注册和启动窗口显示/隐藏逻辑
- [ ] 8.4 集成启动时默认配置创建和索引加载
- [ ] 8.5 集成手动刷新索引入口和失败反馈
- [ ] 8.6 验证开发环境下 macOS 可以运行 Tauri dev 并打开启动窗口

## 9. 文档与 CI

- [ ] 9.1 编写中文 `README.md`，说明依赖安装、本地运行、测试、构建和基础使用
- [ ] 9.2 编写 `docs/architecture.md`，说明模块职责、数据流和 Provider/Action/Adapter 边界
- [ ] 9.3 编写 `docs/development.md`，说明项目结构、常用命令和新增 Provider/Action/Adapter 方法
- [ ] 9.4 编写排错文档，覆盖 Windows Terminal、Linux 终端 fallback、全局快捷键和索引刷新
- [ ] 9.5 配置 GitHub Actions Windows/Linux 标准 runner 检查，不使用 larger runner
- [ ] 9.6 确保普通 push/PR workflow 不构建或上传发布安装包

## 10. 验证与收口

- [ ] 10.1 运行前端 lint、format check、测试和 build
- [ ] 10.2 运行 Rust fmt check、clippy 和测试
- [ ] 10.3 运行统一本地检查命令
- [ ] 10.4 运行 `openspec validate build-quickfox-launcher`
- [ ] 10.5 整理 Windows 手工验收清单，覆盖快捷键、窗口行为、Windows Terminal 执行和权限提示
- [ ] 10.6 检查仓库中没有死代码、未使用生成示例代码或敏感信息
