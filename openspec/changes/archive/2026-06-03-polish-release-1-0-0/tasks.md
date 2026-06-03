## 1. 规约与基线

- [x] 1.1 补充本变更 OpenSpec proposal/design/spec/tasks
- [x] 1.2 复查上一轮未完成的 macOS 权限验收不混入本变更完成判断

## 2. 图标

- [x] 2.1 生成像素风格狐狸 SVG/PNG 图标
- [x] 2.2 配置 Tauri bundle 和托盘使用同一套图标资源

## 3. 网页搜索执行

- [x] 3.1 先写 `bd 1234` 回车直接打开百度 URL 的前端测试
- [x] 3.2 实现前端网页搜索直接执行路径并记录历史

## 4. 历史模式

- [x] 4.1 先写 Shift 进入历史模式、上下浏览、Enter 回填、Escape 退出的 UI 测试
- [x] 4.2 实现独立历史模式，默认上下键只导航搜索结果

## 5. 打开方式

- [x] 5.1 先为 `OpenWithApplication` action 序列化和平台命令构造写 Rust 测试
- [x] 5.2 实现跨平台开发方式打开 Adapter
- [x] 5.3 文件/目录搜索结果右键菜单增加“用开发工具打开”

## 6. Release

- [x] 6.1 更新版本号到 1.0.0
- [x] 6.2 启用 Tauri bundle 配置并补充 release workflow
- [x] 6.3 更新 README/开发文档说明 macOS/Windows release 包构建方式
- [x] 6.4 创建并推送 tag `v1.0.0`，触发 GitHub Release

## 7. 验证

- [x] 7.1 运行 `npm run test`
- [x] 7.2 运行 `npm run rust:test`
- [x] 7.3 运行 `npm run check`
- [x] 7.4 运行 `openspec validate polish-release-1-0-0 --strict`
