## Why

设置页的帮助提示框作为滚动内容区的后代向上展开，会被 `overflow` 裁切，导致问号悬停后提示文字不可见。这直接违反了既有字段级帮助提示需求，也影响其他同类滚动页面。

## What Changes

- 将帮助提示框提升为相对窗口定位的浮层，避免被页面滚动容器裁切。
- 让浮层根据可用空间自动选择上方或下方展示，并收束在窗口左右边界内。
- 保持 hover 与键盘 focus 的访问方式，并补充可访问性关联和位置回归测试。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `launcher-shell`: 完善设置页字段级帮助提示在滚动、缩放与窗口边缘下的完整可见性要求。

## Impact

- 影响 `src/App.tsx` 中的共用帮助图标组件及 `src/styles.css` 的提示框样式。
- 增加前端组件测试；不改变 Rust、配置格式或外部 API。
