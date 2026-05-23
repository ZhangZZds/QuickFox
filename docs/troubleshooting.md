# QuickFox 排错指南

## `npm run tauri dev` 启动但窗口背景不透明

macOS 透明窗口需要同时满足：

- `tauri.conf.json` 中启用 `app.macOSPrivateApi`
- `Cargo.toml` 中为 `tauri` 启用 `macos-private-api` feature

否则会看到整块窗口背景而不是紧凑启动器效果。

## 命令模式只有提示，没有真正执行

先确认两件事：

- 查询以 `>` 开头
- 配置中 `command.enabled = true`

如果仍然没有执行，检查：

- 命令是否被安全规则拦截
- 外部终端是否可用
- macOS 是否允许 Terminal 被脚本唤起

## Linux 终端找不到

Linux 侧按如下顺序尝试终端：

1. `x-terminal-emulator`
2. `gnome-terminal`
3. `konsole`
4. `xfce4-terminal`
5. `xterm`

如果都不存在，命令执行会失败。请安装其中一个终端，或后续扩展首选终端配置。

## Windows Terminal 行为异常

当前 Windows 命令构造默认走：

```text
wt.exe cmd.exe /C <command>
```

若运行失败，请检查：

- `wt.exe` 是否已安装并在 PATH 中
- Windows Terminal 是否被系统策略限制

## 索引刷新后没有结果

优先排查：

- `index.include_dirs` 是否为空
- 目录是否可读
- 是否被 `exclude_dirs` 或 `exclude_patterns` 排除

可以通过手动刷新后的失败报告定位不可读目录。

## 全局快捷键不符合预期

当前仓库已经有双击 Shift 状态边界和窗口显隐边界测试，但真实桌面行为仍需平台手工验收。

若出现异常，请记录：

- 平台与系统版本
- 是否在前台应用中被其他软件占用
- 触发时窗口是否已存在但未聚焦
