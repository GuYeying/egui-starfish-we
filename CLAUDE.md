# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目定位

本 crate 是 **egui-winit 0.37.0 的 vendored fork**（源码逐文件拷贝自 crates.io 发布包，作为独立新 crate 存在）。fork 而非包装的原因：egui-winit 部分模块/内部不对外公开，包装层够不到，只能改源码。

**目标**：starfish-we（SDL 风格 pull 模型：调用方持主循环、`poll_event()` 拉事件）窗口层的 egui 适配层。工作现状（2026-10-24）：**winit 依赖已整体剥离**——上游以 winit 为词汇的路径（回调式事件循环、多视口、IME 组合窗、位图光标、accesskit、窗口几何持久化）全部移除，lib.rs 只剩 starfish-we 单路径：`State::new_we` / `on_we_event` / `take_egui_input_we` / `handle_platform_output_we`；`State` 内部机制（剪贴板、快捷键折叠、键盘/指针/滚轮翻译、macOS 滚动动量）沿用上游。公共 API 不再带 `_we` 后缀重命名是后续可选项（暂保留后缀，减少 example/文档扰动）。

- 本 crate 不在 starfish workspace 内，cargo 命令在本目录直接执行；路径依赖 `../starfish/crates/starfish-we` 指向 starfish 工作区。
- 父项目 starfish（`../starfish`，有自己的 CLAUDE.md，接手先读）：SDL 风格 pull 模型跨平台引擎；**文档/注释/日志用中文**（starfish-we 源码为中英双语）。
- 区分两个 egui-starfish-we：`starfish/crates/egui-starfish-we` 是旧的最小桥接实现（egui 0.32 + src/we.rs）；本目录是新的 vendored fork（现已无 winit）。
- 上游原版基线在本机：`D:\Rust\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\egui-winit-0.37.0\src`。改前/改后都 diff 这份基线——fork 的自有增量应保持有意且最小，勿把无谓漂移混进 fork（否则以后看不出哪些是魔改）。注意：fork 现已整体偏离上游（winit 剥离），diff 时按 NOTICE 的修改清单对账，而非期望逐行贴近。

## 常用命令

```bash
cargo check                        # 类型检查（不链接，裸 bash 可用）
cargo check --no-default-features  # 现在可编译（winit 剥离后不再有"无平台后端编不过"问题）
cargo build / cargo run            # 链接类操作须在 VS 开发者环境执行（见下）
cargo test                         # 单元测试（目前仅 clipboard.rs 内联 #[test]）
cargo test <名字片段>                # 跑单个测试
```

cargo run --example render_test --features render   # egui 联调窗口（链接须走 vs-cargo.bat；example 依赖 feature render）

- **链接须 VS 环境**：裸 bash 下 link.exe 会误解析到 Git 自带 GNU link；助手脚本 `..\starfish\scripts\vs-cargo.bat <cargo 参数…>`（内部 call VsDevCmd.bat 再跑 cargo）。

## 构建状态（2026-10-24 随 winit 剥离更新）

`cargo check`（default / --no-default-features / --features render / --all-features / --example render_test）与 `cargo test`（3 个 clipboard 测试）在 Windows 均通过；`cargo check --target aarch64-apple-ios`（default 与 --no-default-features）亦通过（类型检查，未做真机测试）。需知的构建层差异：

- 平台依赖按上游 target 表门控：objc2*/block2 → macos/ios，smithay-clipboard → Linux/BSD，arboard → 非 Android。windows-sys / wayland-cursor / serde / accesskit_winit 随 winit 路径一并移除。
- `--no-default-features` 现在可编译：上游因 winit 无平台后端编不过，本 fork 无此约束。
- 依赖树已无 winit（`cargo tree | grep winit` 为空）。
- feature 表：default = ["clipboard", "links"]；保留 bytemuck / clipboard / links / wayland / render；accesskit / android-game-activity / android-native-activity / x11 / serde 已随 winit 路径删除。
- **"wayland" 不在 default**：arboard 的 `wayland-data-control` feature 未按平台门控，会把 wl-clipboard-rs/wayland-backend 拉进 iOS 等非 Linux 目标直接编译失败。Linux Wayland 剪贴板主路径 smithay-clipboard 由 "clipboard" 按 target 自带，arboard 该项只是回退路径，需要时显式开 `wayland`。
- arboard 依赖门控为 `not(android, wasm32)`、clipboard.rs 代码门控同步加了 `not(target_arch = "wasm32")`——wasm 上默认 feature 现在可编译（OS 剪贴板不存在，走 egui 应用内模拟回退）。
- iOS：`safe_area.rs`（objc2-ui-kit）接入 `on_we_event` 的 Resized/FocusGained/Resumed 刷新点。**输入链路已在 starfish-we 侧补齐**（2026-10-24）：触摸（Touch + 主手指合成鼠标）、软键盘（隐藏 UITextField 拦截 insertText/deleteBackward → TextInput/Backspace 键对）、硬件按键（UIPress HID → KeyCode）、前后台（Suspended/Resumed），公共 API `Window::set_text_input_visible` 控制软键盘显隐——fork 侧无需改动（合成鼠标与 TextInput 均走既有 we 事件路径）。桥层两个 iOS 目标（device/sim）类型检查通过；**真机/模拟器运行均未测试**（用户指示：测试后置）。

## example/render_test.rs（egui 集成冒烟）

starfish-we 窗口 + starfish-render surface + egui（egui_starfish_we 桥）+ egui-wgpu 同 pass 绘制。帧流程与 0.37 坑位（实测踩过）：

1. 帧序：`surface.begin_frame(Color, depth)`（**必须最先调**，它才填充 color_attachment；false=未就绪跳过本帧）→ `take_egui_input_we` → `ctx.run_ui(raw, |ui| 面板)` → `update_texture`（set 里是 `SmallVec<[ImageDelta;1]>` 要嵌套迭代）→ `update_buffers`（**0.37 强制，不调 render 直接 panic**）→ `begin_render_pass` → 三角形 → `pass.forget_lifetime()` → `renderer.render` → drop → `encoder.finish` → `surface.submit(cmds)`（上传命令先于绘制）→ `surface.present()` → `free_texture` + `textures_delta.clear()`（**不 clear 会 drop panic**）。
2. egui 0.37：面板入口改为 `ctx.run_ui` + `CentralPanel::show(ui, …)`（不再有 `show(ctx, …)`）。
3. egui-wgpu 0.37 的 `render` 要求 `RenderPass<'static>`——已给 starfish-render 的 `RenderPass` 包装加 `forget_lifetime()` 透传（那边的唯一改动）。
4. `begin_render_pass` 槽位类型是 `&Option<ColorAttachment>`（None 元素=跳过槽位）。
5. egui 默认字体无 CJK——`State::load_system_cjk_fonts(&ctx)` 一行搞定（Windows 雅黑/黑体、macOS 苹方、Linux Noto CJK）。
6. 绘制端：`egui_starfish_we::SfEguiRenderer::new(egui_wgpu::Renderer::new(...))` 包装后，`render(&mut pass, …)` 收包装 pass 的可变引用（不转移所有权；内部 unsafe 擦生命周期，契约见 lib.rs SAFETY 注释）；不依赖 starfish-render 时用 `raw_render(&mut raw_pass, …)` 传原生 'static 通道。`update_texture`/`update_buffers`/`free_texture` 经 Deref 照用。

## 许可证与合规（MIT OR Apache-2.0，改大东西时记得同步）

vendored fork 合规件已齐（2026-10-23 落地）：根目录 `LICENSE-MIT` / `LICENSE-APACHE`
（取自上游仓库原文）；根目录 `NOTICE`（衍生声明 + 修改清单——**结构性改动后要更新
这里的清单**）；`Cargo.toml` 标 `license = "MIT OR Apache-2.0"` + `publish = false`；
`lib.rs` 头部有 crate 级衍生声明（Apache §4(b) 变更声明）。上游发布包里没有 LICENSE
文件（其 include 列表引用了仓库级路径但未生效），许可文本取自 GitHub（jsdelivr 镜像，
GitHub 直连在本机不通）。新增第三方依赖时照常由 cargo 权限体系覆盖；再 vendored 别的
crate 时照此模板补三件套。

## 与上游的差异（勿把 vendored 代码当自有逻辑乱改）

- `src/clipboard.rs` 逻辑与上游一致（曾按旧 egui 改 `.pixels()`/`.resolution()`，对 egui `=0.37.0` 不成立，已还原）；2026-10-24 起仅 2 条日志提示的 crate 名改为本 crate。
- `src/safe_area.rs`、`src/macos_scroll_momentum.rs` 逻辑与上游一致；2026-10-24 把文档注释改写为 starfish-we 语境（原注释描述的探测/回避手法——iOS 安全区直读 UIKit、macOS NSEvent 动量监视——依然必要）。safe_area 的调用点从上游 winit 事件路径移到 `on_we_event`。
- `src/lib.rs` 的 fork 增量（皆为有意）：
  - **winit 路径整体移除**：`State::new`/`on_window_event`/`take_egui_input`（winit 版）/`handle_platform_output(_with_event_loop)`、IME 组合窗全套（`on_ime`、`allow_ime`、Windows VK_PROCESSKEY 规避）、位图光标（`CustomCursor`/`apply_cursor`）、accesskit、多视口命令与建窗系（`process_viewport_commands`/`create_window`/`create_winit_window_attributes`/`apply_viewport_builder_to_window`/`set_maximized` 等）、`update_viewport_info`/`inner_rect_in_points`/`outer_rect_in_points`、`short_*_event_description`、`key_from_winit_key`/`key_from_named_key`/`key_from_key_code`（winit 键表）、`translate_cursor`、`to_egui_touch_phase`/`to_egui_theme`。
  - 键表合并：`translate_we_key_code` + `key_from_key_code` → `we_key_to_egui_key` 一张直通表（`MetaLeft/Right` → `SuperLeft/Right`；winit 独有的 F13+/浏览器/媒体/launch 键 starfish-we 无对应，不映射）。
  - `translate_mouse_button`（winit 版）删除；`translate_we_mouse_button` 改为直出 egui `PointerButton`，并新增 Back/Forward → Extra1/Extra2（旧版丢弃）。注意 `starfish_we::MouseButton` 是 `#[non_exhaustive]`，match 须留通配臂。
  - `on_mouse_button_input` 签名 egui 化（`pressed: bool` + `egui::PointerButton`）。
  - `screen_size_in_pixels` / `pixels_per_point` 收 `starfish_we::Window`（`we_pixels_per_point` 坐正为 `pixels_per_point`）；`State::new_inner` 折叠进 `new_we`。
  - iOS 安全区刷新点接入 `on_we_event`（Resized/FocusGained/Resumed）。
  - 删 crate 级文档与一行 clippy expect；恢复 `pub mod clipboard;`；feature `render` 的 `SfEguiRenderer`（egui_wgpu::Renderer 包装：`render` 收 starfish-render 包装 pass、`raw_render` 收原生 `RenderPass<'static>`、Deref/DerefMut 透传——egui-wgpu 0.37 硬性要求 'static 通道，lib.rs 内一次受控 unsafe（transmute 擦生命周期）桥接——安全不变式（raw 绝不离函数体等三条）写死在 SAFETY 注释里，改动前必读；starfish-render 侧只需提供 `inner`/`inner_mut` 访问器）；`load_system_cjk_fonts`；文件头部有衍生与许可声明。
  - 从上游 winit 路径抽出、现由 we 单路径使用的助手：`on_key_and_text`（剪贴板快捷键折叠 + Key/Text 发射）、`on_cursor_moved_points`、`on_wheel`（滚轮主体，含 macOS 动量判定）。`State` 保留字段 `simulate_touch_screen`（上游调试开关，无公共 setter，恒 false）。
- 文件删除：`src/window_settings.rs`（eframe 窗口几何持久化，纯 winit）、`src/dropped_file.rs`（starfish-we v1 无 drag-drop 事件，无生产者）。`pub use window_settings::…` 同步移除；feature serde 一并删除。
- 上游的行为语义（剪贴板模拟回退、`EventResponse` consumed/repaint 语义）对 fork 依然成立；IME 组合、多视口、位图光标、文件拖放语义随 winit 路径移除（starfish-we v1 缺口，见 lib.rs 翻译层模块文档）。

## 架构

fork 各文件职责：

- **`src/lib.rs`**（约 1000 行）：核心是 `State`——把 starfish-we `Event` 翻译并累积进 `egui::RawInput`（`on_we_event`），`take_egui_input_we` 交帧，`handle_platform_output_we` 收尾（光标可见性、剪贴板、链接）；内部助手 `on_key_and_text` / `on_cursor_moved_points` / `on_wheel`（上游抽出）；翻译自由函数 `we_key_to_egui_key` / `translate_we_mouse_button`；另有 feature `render` 的 `SfEguiRenderer` 与便利函数 `load_system_cjk_fonts`、`screen_size_in_pixels` / `pixels_per_point`。`EventResponse`（`consumed`/`captured` 语义同上游：egui 是否吃掉事件、是否需要重绘）。
- **starfish-we 翻译层**（lib.rs 中段，定约见其模块文档）：starfish-we → egui 翻译。定约：事件坐标 = 物理像素，points = 物理像素 ÷（`egui_ctx.zoom_factor() × window.dpi_scale()`，未开 `HIGH_PIXEL_DENSITY` 时退化为 1:1）；`KeyCode` 即 W3C code，经 `we_key_to_egui_key` 直通；文本走 `TextInput`（过滤控制字符与命令修饰键）；修饰键随 KeyDown/Up 携带、更新副本并广播 `ModifiersChanged`。v1 缺口：无 MouseLeave（hover 滞留）、IME/位图光标不落窗、文件拖放无事件、`Touch` 忽略（后端已按 SDL 惯例合成鼠标）。用法：
  ```rust
  let mut state = egui_starfish_we::State::new_we(ctx.clone(), ViewportId::ROOT, None, &window);
  loop {
      while let Some(ev) = window.poll_event() {
          if matches!(ev, starfish_we::Event::CloseRequested) { break; }
          state.on_we_event(&window, &ev);
      }
      let _raw = state.take_egui_input_we(&window);   // ctx.begin_pass(raw) → ui → end_pass
      // state.handle_platform_output_we(&window, out.platform_output);
      starfish_we::next_frame().await;
  }
  ```
- **`src/clipboard.rs`**：arboard（Win/Mac）+ smithay-clipboard（Wayland）+ 无 OS 剪贴板时的 egui 内部模拟回退；唯一有单元测试的文件。Wayland display 句柄在 `new_we` 里从 `we_window.raw_handle()` 提取。
- **`src/safe_area.rs`**：iOS 刘海/安全区 inset（objc2-ui-kit 直读；等 starfish-we 提供对等 API 后可并入）。
- **`src/macos_scroll_momentum.rs`**：macOS 滚动动量修补（NSEvent 本地监视器；starfish-we `MouseWheel` 无 source/phase，仍必要）。

**迁移史（备忘）**：egui-winit 假定 winit 回调式事件循环（`ActiveEventLoop` 驱动），starfish-we 是 pull 模型（调用方持 `while` 主循环、`window.poll_event()` 拉取 `starfish_we::Event`，平台实现在 starfish-we `src/platform/` 横向 cfg）。适配 = 给 `State` 换事件入口：把 starfish-we `Event` 翻译进 `RawInput` 累积 + `EventResponse` 回传调用方。2026-10-23 we/winit 双路径共存；2026-10-24 winit 路径整体拆除。
