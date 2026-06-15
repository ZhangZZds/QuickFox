## Why

当前 Esc 只绑定在搜索输入框的键盘事件上。当焦点落在结果项、右键动作菜单、设置页快捷键录制按钮或其它控件上时，用户按 Esc 可能没有可见效果。QuickFox 需要定义 Esc 在不同 UI 场景下的优先级，并保证焦点变化后仍能执行预期退出行为。

## What Changes

- 定义 Esc 的逐层退出语义：先退出局部状态，再关闭启动器。
- 动作菜单打开时，Esc 关闭动作菜单而不关闭启动器。
- 历史模式打开时，Esc 退出历史模式而不关闭启动器。
- 快捷键录制中，Esc 取消录制并保留原快捷键。
- 搜索/结果/命令预览等普通启动器状态下，Esc 关闭启动器且不执行动作、不记录历史。
- 设置页中的轻量弹层打开时，Esc 关闭弹层；设置页常态下不因 Esc 直接关闭窗口。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `launcher-shell`: 明确 Esc 键在启动器、动作菜单、历史模式、命令预览和设置页局部状态中的行为。

## Impact

- 前端 `src/App.tsx` 需要把 Esc 处理从输入框局部逻辑提升为启动器/设置页级别的统一处理。
- 前端测试 `src/App.test.tsx` 需要覆盖焦点不在输入框时的 Esc 行为，以及局部状态的退出优先级。
- 不改变 Rust core、Provider、Action 或平台 Adapter。
