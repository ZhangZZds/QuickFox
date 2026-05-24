# macOS 手工验收结果

变更：`fix-launcher-manual-qa-bugs`

记录时间：2026-05-24 15:47 CST

## 本机已验证

- `npm run check` 通过，覆盖前端格式化、ESLint、Vitest、构建、Rust
  `rustfmt`、`clippy` 和 Rust 单元测试。
- `openspec validate fix-launcher-manual-qa-bugs --strict` 通过。
- `npm run tauri dev` 能完成 Vite 与 Cargo 编译，并启动调试版
  `target/debug/quickfox`。
- 调试版启动后全局键盘监听会初始化；当前机器未授权输入监控或辅助功能权限时，
  会记录监听禁用原因，应用本体仍能启动。
- 调试进程已清理，未留下 Vite、Tauri dev 或 `target/debug/quickfox`
  后台进程。

## 已自动化覆盖但仍建议桌面复验

- 全局键盘监听已接入 `keytap`，后端会监听左右 Shift 的 key-down 事件，并在
  双击 Shift 时复用同一套窗口显示、置前、隐藏逻辑。
- 空输入不显示结果区域留白。
- 快速启动窗口内不显示设置按钮。
- 托盘设置事件能切换到设置页。
- 设置页按“搜索与索引”“网页搜索”“历史”“命令执行”“外观与窗口”
  分组。
- 设置页刷新索引会调用 Rust 侧增量刷新命令并显示反馈。
- `g 1234`、`bd 1234` 使用空格前缀解析为网页搜索。
- Enter 执行文件、目录、命令或网页搜索后才记录输入历史；Esc 或未执行不记录。
- 上/下方向键能召回最近输入历史。
- 右键菜单使用点击坐标定位。
- 结果列表使用内部滚动容器，隐藏页面级割裂滚动条。
- 普通模糊搜索对低相关结果设置阈值，避免 `Openspec_123` 返回
  PyCharm 内部资源。
- `.app`、`.exe`、`.lnk`、`.desktop` 识别为应用类型；macOS `.app`
  包内部内容不会进入普通搜索结果。
- 排序优先级为应用程序 > 文件 > 目录。

## 阻塞或需要人工确认

- 真实全局 `Shift+Shift` 需要在 macOS 授权输入监控或辅助功能权限后人工确认。
- 已评估 `tauri-plugin-global-shortcut` / `global-hotkey`：该方案只覆盖普通
  accelerator，例如修饰键加普通键，不能注册“单独 Shift”或“双击 Shift”；
  因此当前实现改用 observe-only 低层键盘 tap。
- 托盘菜单“显示 QuickFox”“设置”需要在 macOS 菜单栏中人工点击确认。
- 打开 URL、打开文件/目录、通过 macOS Terminal 执行命令需要人工确认真实
  系统动作。

## 下一步决策

要完整关闭 QF-001，需要完成一次真实 macOS 桌面验收：授权 QuickFox 输入监控
或辅助功能权限后，验证隐藏、后台、前台三个窗口状态下双击 Shift 的行为。
