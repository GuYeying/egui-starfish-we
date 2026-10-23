# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目定位

本 crate 是 **egui-winit 0.37.0 的 vendored fork**（源码逐文件拷贝自 crates.io 发布包，作为独立新 crate 存在）。fork 而非包装的原因：egui-winit 部分模块/内部不对外公开，包装层够不到，只能改源码。

**目标**：魔改 `src/lib.rs`，让使用风格符合 starfish-we（SDL 风格 pull 模型：调用方持主循环、`poll_event()` 拉事件），同时**尽可能保持 lib.rs 的内部结构**（`State` 及其方法布局沿用上游）。工作现状（2026-10-23）：**we 路径 v1 已接线**（全部并入 lib.rs 末段，刻意不拆独立文件）——`State::new_we` / `on_we_event` / `take_egui_input_we` / `handle_platform_output_we`，内部复用 `State` 全部机制；winit 路径暂保留共存；`wgpu` 依赖仍未使用。

- 本 crate 不在 starfish workspace 内，cargo 命令在本目录直接执行；路径依赖 `../starfish/crates/starfish-we` 指向 starfish 工作区。
- 父项目 starfish（`../starfish`，有自己的 CLAUDE.md，接手先读）：SDL 风格 pull 模型跨平台引擎；**文档/注释/日志用中文**（starfish-we 源码为中英双语）。
- 区分两个 egui-starfish-we：`starfish/crates/egui-starfish-we` 是旧的最小桥接实现（egui 0.32 + src/we.rs，"starfish-we 事件 → egui::RawInput 翻译桥"）；本目录是新的全量 vendored fork，前者可作意图参考。
- 上游原版基线在本机：`D:\Rust\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\egui-winit-0.37.0\src`。改前/改后都 diff 这份基线——fork 的自有增量应保持有意且最小，勿把无谓漂移混进 fork（否则以后看不出哪些是魔改）。

## 常用命令

```bash
cargo check              # 类型检查（不链接，裸 bash 可用）
cargo build / cargo run  # 链接类操作须在 VS 开发者环境执行（见下）
cargo test               # 单元测试（目前仅 clipboard.rs 内联 #[test]）
cargo test <名字片段>     # 跑单个测试
```

cargo run --example render_test --features render   # egui 联调窗口（链接须走 vs-cargo.bat；example 依赖 feature render）

- **链接须 VS 环境**：裸 bash 下 link.exe 会误解析到 Git 自带 GNU link；助手脚本 `..\starfish\scripts\vs-cargo.bat <cargo 参数…>`（内部 call VsDevCmd.bat 再跑 cargo）。

## 构建状态（2026-10-23 修通）

`cargo check` / `cargo test`（3 个 clipboard 测试）在 Windows 均通过。为跑通所做、需知的构建层差异：

- 平台依赖已按上游 target 表门控：objc2*/block2 → macos/ios，smithay-clipboard/wayland-cursor → Linux/BSD，arboard → 非 Android（含 image-data），windows-sys → windows。
- `winit = "0.30.13", default-features = false`：src 按 0.30 API 编写，后端由 default feature 的 `winit/default` 重开（同上游）；原 `"0.29"` 系笔误。
- `pub mod clipboard;` 已恢复（lib.rs 引用 `clipboard::Clipboard`）。
- clipboard.rs 已还原为上游逐字节版本：egui `0.37.0` 的 `ColorImage.pixels` 是字段不是方法，fork 里 `.pixels()`/`.resolution()` 的改法编译不过。
- 注意 `--no-default-features` 时 winit 无平台后端编不过——上游设计如此，由宿主（eframe 类）开 `winit/default`。
- 遗留：registry 的 `egui-winit = "0.37"` 仍在依赖里（未被 src 引用，纯过渡；迁移稳定后应移除）；`wgpu` 主依赖现被 `SfEguiRenderer::raw_render` 签名引用。

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

- `src/clipboard.rs` 与上游逐字节一致（曾按旧 egui 改 `.pixels()`/`.resolution()`，对 egui `=0.37.0` 不成立，已还原）。
- `src/lib.rs` 的 fork 增量（皆为有意）：删 crate 级文档与一行 clippy expect；恢复 `pub mod clipboard;`；`State::new` 拆出 `new_inner`（display 句柄参数化，供 we 路径复用，Clipboard 的 Wayland display 由 we 路径传 `wl_display*`）；从 winit 路径抽取三个 winit/we 共用助手：`on_key_and_text`（键盘 egui 域主体：剪贴板快捷键折叠 + Key/Text 发射）、`on_cursor_moved_points`、`on_wheel`（滚轮主体，含 macOS 动量判定）；文件末段并入 starfish-we 翻译层（we 专属 `impl State` 块 + `translate_we_key_code` / `translate_we_mouse_button` / `we_pixels_per_point` / `load_system_cjk_fonts` 自由函数，有 `// ---` 分隔线）；另有 feature `render` 的 `SfEguiRenderer`（egui_wgpu::Renderer 包装：`render` 收 starfish-render 包装 pass、`raw_render` 收原生 `RenderPass<'static>`、Deref/DerefMut 透传——egui-wgpu 0.37 硬性要求 'static 通道，`inner_mut()` 的借用给不了，lib.rs 内用一次受控 unsafe（transmute 擦生命周期）桥接——安全不变式（raw 绝不离函数体等三条）写死在 SAFETY 注释里，改动前必读；starfish-render 侧只需提供 `inner`/`inner_mut` 访问器）；文件头部有衍生与许可声明。
- 其余三个文件（dropped_file / safe_area / macos_scroll_momentum）与上游逐字节一致。注释里的 IME 测试矩阵、TODO(emilk) 均为上游原文；上游的行为语义（IME、多视口、剪贴板模拟回退）对 fork 依然成立。

## 架构

vendored egui-winit 各文件职责：

- **`src/lib.rs`**（约 2700 行，主体）：核心是 `State`——把 winit `WindowEvent` 翻译并累积进 `egui::RawInput`（`on_window_event`），`take_egui_input` 交帧，`handle_platform_output(_with_event_loop)` 收尾（光标、剪贴板、IME、虚拟键盘）；`process_viewport_commands` / `create_window` / `apply_viewport_builder_to_window` 支撑 egui 多视口；另有自由函数 `screen_size_in_pixels` / `pixels_per_point` / `update_viewport_info` 等，及 `EventResponse`（`consumed`/`captured`：egui 是否吃掉事件、指针是否落在 egui 面板上）。
- **starfish-we 翻译层**（lib.rs 末段，魔改主体）：starfish-we → egui 翻译。定约：事件坐标 = 物理像素，points = 物理像素 ÷（`egui_ctx.zoom_factor() × window.dpi_scale()`，未开 `HIGH_PIXEL_DENSITY` 时退化为 1:1）；`KeyCode` 即 W3C code，与 winit `keyboard::KeyCode` 逐名直通后复用上游 `key_from_key_code`（注意 winit 的 Win 键叫 `SuperLeft/Right`，starfish-we 叫 `MetaLeft/Right`）；文本走 `TextInput`（过滤控制字符与命令修饰键）；修饰键随 KeyDown/Up 携带、更新副本并广播 `ModifiersChanged`。v1 缺口：无 MouseLeave（hover 滞留）、IME/位图光标不落窗、`Touch` 忽略（后端已按 SDL 惯例合成鼠标）。用法：
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
- **`src/window_settings.rs`**：`WindowSettings`/`WindowGeometry`，feature `serde` 时序列化持久化窗口几何。
- **`src/clipboard.rs`**：arboard（Win/Mac）+ smithay-clipboard（Wayland）+ 无 OS 剪贴板时的 egui 内部模拟回退；唯一有单元测试的文件。
- **`src/dropped_file.rs`**：`NativeFile` 实现 `egui::DroppedFile` trait（懒读盘）。
- **`src/safe_area.rs`**：iOS 刘海/安全区 inset（等 winit 0.31 后可删，见注释）。
- **`src/macos_scroll_momentum.rs`**：macOS 滚动动量修补（等 winit PR #4732，见 lib.rs 顶部 TODO）。

**迁移的本质难点——控制流模型相反**：egui-winit 假定 winit 回调式事件循环（`ActiveEventLoop` 驱动），而 starfish-we 是 pull 模型——调用方持 `while` 主循环、`window.poll_event()` 拉取 `starfish_we::Event`（`CloseRequested`/`Resized`/`KeyDown{key,modifiers,repeat}`/…，平台实现在 starfish-we `src/platform/` 横向 cfg）。适配即给 `State` 换事件入口：把 starfish-we `Event` 翻译进 `RawInput` 累积（结构上贴着上游 `on_window_event` 的分派形态做），并为 `EventResponse.captured` 找一条回到 starfish-we 调用方的通路。
