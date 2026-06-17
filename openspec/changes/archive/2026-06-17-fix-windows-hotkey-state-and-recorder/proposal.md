## Why

Windows 上使用 `Shift+Space` 作为全局唤醒键时，普通打字按下 Space 可能误触发 QuickFox。设置页录制 `Alt+Space` 等系统保留组合键时也缺少可操作反馈，导致只有真实使用时才暴露问题。

## What Changes

- 修正全局组合键状态机，避免修饰键状态在释放事件丢失、焦点切换或系统快捷键截获后残留并误触发后续普通按键。
- 改进设置页快捷键录制，对已知高风险或系统保留组合键给出提示，不把不可可靠捕获的组合静默当作成功。
- 补充 Rust 状态机单元测试、前端录制测试和 Windows 手工/自动化探索清单。

## Capabilities

### New Capabilities

### Modified Capabilities

- `launcher-shell`: 增加全局唤醒组合键防误触、快捷键录制冲突反馈和桌面交互探索要求。

## Impact

- Rust core: `HotkeyState` 组合键识别逻辑和相关单元测试。
- Frontend: 设置页全局唤醒键录制器、提示文案和 React 测试。
- Docs/OpenSpec: Windows 桌面手工验收和可自动化探索策略。
