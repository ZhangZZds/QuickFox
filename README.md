# QuickFox

QuickFox 是一个基于 Tauri 的跨平台快速启动器，灵感来自 Listary。

当前仓库采用 OpenSpec + Superpowers 的规格驱动流程开发：

- 前端：React + Vite + TypeScript
- 后端：Rust + Tauri
- 测试：Vitest、Rust unit tests

## 当前能力

- 文件和目录索引扫描
- 普通模糊搜索与显式正则搜索
- 计算器结果
- 显式网页搜索前缀，例如 `g 关键词` 和 `bd 关键词`
- 命令模式预览与安全检查
- 文件/目录打开、打开所在目录、复制路径、用开发工具打开
- macOS 菜单栏图标与启动窗口

## 环境要求

- Node.js 20+
- npm 10+
- Rust stable
- Tauri 2 所需系统依赖

macOS 一般不需要额外系统库；Linux 请参考 Tauri 依赖安装文档补齐 WebKitGTK 等依赖。

## 安装依赖

```bash
npm install
```

## 本地开发

启动前端开发服务器：

```bash
npm run dev
```

启动完整 Tauri 桌面应用：

```bash
npm run tauri dev
```

## 测试与检查

前端测试：

```bash
npm run test
```

Rust 测试：

```bash
npm run rust:test
```

完整本地检查：

```bash
npm run check
```

OpenSpec 校验：

```bash
openspec validate build-quickfox-launcher
```

## 构建

只构建前端静态资源：

```bash
npm run build
```

构建本机 Tauri release 包：

```bash
npm run tauri build
```

普通 push / PR 只跑检查，不发布安装包。发布 macOS / Windows 安装包通过 GitHub
tag workflow 触发：

```bash
git tag v1.1.0
git push origin v1.1.0
```

GitHub Actions 会在 `macos-latest` 和 `windows-latest` 上运行 release workflow，
并把 Tauri bundle 上传到 GitHub Release。

## 基础使用

- 打开启动器后直接输入关键字搜索文件或目录
- 输入 `re:` 前缀进入显式正则搜索，例如 `re:.*\\.md$`
- 输入网页前缀，例如 `g tauri v2` 或 `bd tauri v2`
- 输入 `>` 前缀进入命令模式，例如 `> git status`
- 按 `Shift` 查看最近输入历史，历史模式里用上下键选择、Enter 回填
- 右键结果可执行二级动作

## 安全说明

- 命令执行默认关闭
- 危险命令识别只是防护栏，不是沙箱
- 命令以当前用户权限在外部终端中运行

更多实现细节见：

- [docs/architecture.md](/Users/frankzhang/workspace/QuickFox/docs/architecture.md)
- [docs/development.md](/Users/frankzhang/workspace/QuickFox/docs/development.md)
- [docs/troubleshooting.md](/Users/frankzhang/workspace/QuickFox/docs/troubleshooting.md)
