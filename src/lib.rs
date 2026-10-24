//! egui-starfish-we：starfish-we 拉模型窗口层的 egui 输入适配层。
//!
//! 衍生与许可声明（Apache-2.0 §4(b) 变更声明 / MIT 声明保留）：
//! - 原始代码衍生自 egui-winit 0.37.0（<https://github.com/emilk/egui>），
//!   Copyright (c) Emil Ernerfeldt and egui contributors，MIT OR Apache-2.0 双许可；
//!   两份许可全文见本目录 `LICENSE-MIT` / `LICENSE-APACHE`（文本未改动）。
//! - 本 crate 沿用同一 MIT OR Apache-2.0 双许可；修改清单见 `NOTICE` 文件与
//!   CLAUDE.md「与上游的差异」。
//! - 2026-10-24 起 winit 依赖整体剥离：上游以 winit `Window` / `WindowEvent`
//!   为词汇的路径（回调式事件循环、多视口命令、IME 组合窗、位图光标、
//!   accesskit、窗口几何持久化等）已移除。本 crate 只认 starfish-we 的
//!   pull 模型：`Window::poll_event()` → [`State::on_we_event`] →
//!   [`State::take_egui_input_we`] → UI → [`State::handle_platform_output_we`]。
//!   `State` 内部机制（剪贴板、快捷键折叠、键盘/指针/滚轮翻译、macOS 滚动
//!   动量判定）沿用上游。
//!
//! 用法见 [`State::new_we`] 与 `example/render_test.rs`。

#[cfg(target_os = "macos")]
mod macos_scroll_momentum;
pub mod clipboard;

mod safe_area;

pub use egui;
use egui::ViewportId;

use std::ptr::NonNull;

// macOS 滚动动量：starfish-we 的 `MouseWheel` 不携带 phase/source，本模块的
// NSEvent 监视器仍是 macOS 动量判定的必要手段（移植自上游，逻辑未改）。
#[cfg(target_os = "macos")]
use macos_scroll_momentum::ScrollMomentumMonitor;

/// 窗口客户区尺寸（物理像素）。
///
/// iOS 注记：上游用 `outer_size` 以把灵动岛区域计入（交给 eframe 用户避让）；
/// starfish-we 只有客户区尺寸，灵动岛避让退化为 [`egui::RawInput::safe_area_insets`]
/// （见 [`State::on_we_event`] 的 iOS 刷新点）。
pub fn screen_size_in_pixels(window: &starfish_we::Window) -> egui::Vec2 {
    let (width, height) = window.size();
    egui::vec2(width as f32, height as f32)
}

/// Calculate the `pixels_per_point` for a given window, given the current egui zoom factor.
///
/// we 域定约：物理像素 ÷（egui 缩放 × 窗口 DPI）= points；
/// 未开 `HIGH_PIXEL_DENSITY` 时 dpi_scale = 1，退化为 1:1。
pub fn pixels_per_point(egui_ctx: &egui::Context, window: &starfish_we::Window) -> f32 {
    egui_ctx.zoom_factor() * window.dpi_scale()
}

// ----------------------------------------------------------------------------

#[must_use]
#[derive(Clone, Copy, Debug, Default)]
pub struct EventResponse {
    /// If true, egui consumed this event, i.e. wants exclusive use of this event
    /// (e.g. a mouse click on an egui window, or entering text into a text field).
    ///
    /// For instance, if you use egui for a game, you should only
    /// pass on the events to your game when [`Self::consumed`] is `false`.
    ///
    /// Note that egui uses `tab` to move focus between elements, so this will always be `true` for tabs.
    pub consumed: bool,

    /// Do we need an egui refresh because of this event?
    pub repaint: bool,
}

// ----------------------------------------------------------------------------

/// Handles the integration between egui and a starfish-we `Window`.
///
/// Instantiate one of these per window.
pub struct State {
    /// Shared clone.
    egui_ctx: egui::Context,

    viewport_id: ViewportId,
    start_time: web_time::Instant,
    egui_input: egui::RawInput,

    /// The current modifier state.
    ///
    /// We keep a copy so we can stamp
    /// it onto per-event `modifiers` fields and emit [`egui::Event::ModifiersChanged`].
    modifiers: egui::Modifiers,

    pointer_pos_in_points: Option<egui::Pos2>,
    any_pointer_button_down: bool,
    current_cursor_icon: Option<egui::CursorIcon>,

    clipboard: clipboard::Clipboard,

    /// If `true`, mouse inputs will be treated as touches.
    /// Useful for debugging touch support in egui.
    ///
    /// Creates duplicate touches, if real touch inputs are coming.
    simulate_touch_screen: bool,

    /// See [`Self::set_clipboard_shortcuts`].
    clipboard_shortcuts: bool,

    // macOS 动量判定：窗口层（上游 winit、今 starfish-we）都不报告滚动的
    // source/phase，只能靠本 NSEvent 监视器（见 macos_scroll_momentum.rs）。
    #[cfg(target_os = "macos")]
    scroll_momentum_monitor: Option<ScrollMomentumMonitor>,
}

impl State {
    /// starfish-we 路径的构造入口（定约见文件末「starfish-we 翻译层」文档）。
    ///
    /// ```ignore
    /// let mut state = egui_starfish_we::State::new_we(ctx.clone(), ViewportId::ROOT, None, &window);
    /// loop {
    ///     while let Some(ev) = window.poll_event() {
    ///         if matches!(ev, starfish_we::Event::CloseRequested) { break; }
    ///         state.on_we_event(&window, &ev);
    ///     }
    ///     let _raw = state.take_egui_input_we(&window);   // ctx.begin_pass(raw) → ui → end_pass
    ///     // state.handle_platform_output_we(&window, out.platform_output);
    ///     starfish_we::next_frame().await;
    /// }
    /// ```
    pub fn new_we(
        egui_ctx: egui::Context,
        viewport_id: ViewportId,
        max_texture_side: Option<usize>,
        we_window: &starfish_we::Window,
    ) -> Self {
        profiling::function_scope!();

        let egui_input = egui::RawInput {
            focused: false, // FocusGained/FocusLost 事件会告知我们
            ..Default::default()
        };

        // Wayland 下把 wl_display* 交给 Clipboard 以启用 OS 剪贴板；其余平台
        // None（arboard 自理；smithay 路径无 display 会回退到应用内模拟剪贴板）。
        let display = match we_window.raw_handle() {
            starfish_we::RawWindowHandle::Wayland { display, .. } => NonNull::new(display).map(
                |display| {
                    raw_window_handle::RawDisplayHandle::Wayland(
                        raw_window_handle::WaylandDisplayHandle::new(display),
                    )
                },
            ),
            _ => None,
        };

        // SAFETY: `display` 指针的有效性由调用方保证：取自 starfish-we `Window`
        // 的裸句柄（指针在 Window 存活期内有效，见 starfish-we handle.rs）。
        #[expect(unsafe_code)]
        let clipboard = unsafe { clipboard::Clipboard::new(display) };

        let mut slf = Self {
            viewport_id,
            start_time: web_time::Instant::now()
                .checked_sub(web_time::Duration::from_secs_f64(egui_ctx.time()))
                .unwrap_or_else(web_time::Instant::now),
            egui_ctx,
            egui_input,
            modifiers: egui::Modifiers::default(),
            pointer_pos_in_points: None,
            any_pointer_button_down: false,
            current_cursor_icon: None,

            clipboard,

            simulate_touch_screen: false,

            clipboard_shortcuts: true,

            #[cfg(target_os = "macos")]
            scroll_momentum_monitor: ScrollMomentumMonitor::install(),
        };

        if let Some(max_texture_side) = max_texture_side {
            slf.set_max_texture_side(max_texture_side);
        }
        slf
    }

    /// Call this once a graphics context has been created to update the maximum texture dimensions
    /// that egui will use.
    pub fn set_max_texture_side(&mut self, max_texture_side: usize) {
        self.egui_input.max_texture_side = Some(max_texture_side);
    }

    /// Fetches text from the clipboard and returns it.
    pub fn clipboard_text(&mut self) -> Option<String> {
        self.clipboard.get()
    }

    /// Fetches an image from the clipboard and returns it, if there is one and the platform
    /// backend supports it. Mirrors [`Self::clipboard_text`] for images.
    pub fn clipboard_image(&mut self) -> Option<egui::ColorImage> {
        self.clipboard.get_image()
    }

    /// Places the text onto the clipboard.
    pub fn set_clipboard_text(&mut self, text: String) {
        self.clipboard.set_text(text);
    }

    /// Are clipboard keyboard shortcuts translated into clipboard events?
    ///
    /// See [`Self::set_clipboard_shortcuts`].
    #[inline]
    pub fn clipboard_shortcuts(&self) -> bool {
        self.clipboard_shortcuts
    }

    /// Should the platform clipboard keyboard shortcuts be translated into
    /// [`egui::Event::Cut`], [`egui::Event::Copy`] and [`egui::Event::Paste`]?
    ///
    /// The shortcuts are <kbd>Cmd/Ctrl</kbd>+<kbd>X</kbd>/<kbd>C</kbd>/<kbd>V</kbd>,
    /// the dedicated Cut/Copy/Paste keys, and on Windows also
    /// <kbd>Shift</kbd>+<kbd>Delete</kbd>, <kbd>Ctrl</kbd>+<kbd>Insert</kbd> and <kbd>Shift</kbd>+<kbd>Insert</kbd>.
    ///
    /// If `false`, these key presses are instead passed on as ordinary [`egui::Event::Key`] events,
    /// so your app can bind them to whatever it wants.
    /// Note that this means built-in widgets such as [`egui::TextEdit`] will no longer
    /// respond to these shortcuts, since they listen for the clipboard events.
    /// You can still push [`egui::Event::Cut`] etc. yourself.
    ///
    /// Default: `true`.
    #[inline]
    pub fn set_clipboard_shortcuts(&mut self, enabled: bool) {
        self.clipboard_shortcuts = enabled;
    }

    #[inline]
    pub fn egui_ctx(&self) -> &egui::Context {
        &self.egui_ctx
    }

    /// The current input state.
    /// This is changed by [`Self::on_we_event`] and cleared by [`Self::take_egui_input_we`].
    #[inline]
    pub fn egui_input(&self) -> &egui::RawInput {
        &self.egui_input
    }

    /// The current input state.
    /// This is changed by [`Self::on_we_event`] and cleared by [`Self::take_egui_input_we`].
    #[inline]
    pub fn egui_input_mut(&mut self) -> &mut egui::RawInput {
        &mut self.egui_input
    }

    /// Forward raw mouse motion (e.g. from a relative-mouse / raw-input API)
    /// to egui as [`egui::Event::MouseMoved`].
    ///
    /// The motion is ignored unless the pointer is inside the window or a pointer button is held down,
    /// so that mouse motion elsewhere on the screen doesn't cause repaints.
    ///
    /// Returns `true` if the event was sent to egui.
    pub fn on_mouse_motion(&mut self, delta: (f64, f64)) -> bool {
        if !self.is_pointer_in_window() && !self.any_pointer_button_down {
            return false;
        }

        self.egui_input
            .events
            .push(egui::Event::MouseMoved(egui::Vec2 {
                x: delta.0 as f32,
                y: delta.1 as f32,
            }));
        true
    }

    /// Returns `true` when the pointer is currently inside the window.
    pub fn is_pointer_in_window(&self) -> bool {
        self.pointer_pos_in_points.is_some()
    }

    /// Returns `true` if any pointer button is currently held down.
    pub fn is_any_pointer_button_down(&self) -> bool {
        self.any_pointer_button_down
    }

    fn on_mouse_button_input(&mut self, pressed: bool, button: egui::PointerButton) {
        if let Some(pos) = self.pointer_pos_in_points {
            self.egui_input.events.push(egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers: self.modifiers,
            });

            // Middle-click pastes the PRIMARY selection on X11 and Wayland:
            if pressed
                && button == egui::PointerButton::Middle
                && let Some(text) = self.clipboard.get_primary_text()
                && !text.is_empty()
            {
                self.egui_input
                    .events
                    .push(egui::Event::MiddleClickPaste { pos, text });
            }

            if self.simulate_touch_screen {
                if pressed {
                    self.any_pointer_button_down = true;

                    self.egui_input.events.push(egui::Event::Touch {
                        device_id: egui::TouchDeviceId(0),
                        id: egui::TouchId(0),
                        phase: egui::TouchPhase::Start,
                        pos,
                        force: None,
                    });
                } else {
                    self.any_pointer_button_down = false;

                    self.egui_input.events.push(egui::Event::PointerGone);

                    self.egui_input.events.push(egui::Event::Touch {
                        device_id: egui::TouchDeviceId(0),
                        id: egui::TouchId(0),
                        phase: egui::TouchPhase::End,
                        pos,
                        force: None,
                    });
                }
            }
        }
    }

    /// 光标移动的 points 域主体（starfish-we 单入口；沿用上游抽出的助手）。
    fn on_cursor_moved_points(&mut self, pos_in_points: egui::Pos2) {
        self.pointer_pos_in_points = Some(pos_in_points);

        if self.simulate_touch_screen {
            if self.any_pointer_button_down {
                self.egui_input
                    .events
                    .push(egui::Event::PointerMoved(pos_in_points));

                self.egui_input.events.push(egui::Event::Touch {
                    device_id: egui::TouchDeviceId(0),
                    id: egui::TouchId(0),
                    phase: egui::TouchPhase::Move,
                    pos: pos_in_points,
                    force: None,
                });
            }
        } else {
            self.egui_input
                .events
                .push(egui::Event::PointerMoved(pos_in_points));
        }
    }

    /// 滚轮主体的 egui 域翻译（source 含 macOS 动量判定；沿用上游抽出的助手）。
    fn on_wheel(&mut self, unit: egui::MouseWheelUnit, delta: egui::Vec2, phase: egui::TouchPhase) {
        {
            let modifiers = self.modifiers;

            // The window layer (upstream: winit; here: starfish-we) doesn't tell us what
            // is driving the scroll, and collapses the macOS momentum phase, so we
            // detect that ourselves via a local `NSEvent` monitor.
            // TODO(emilk): 上游等 winit PR #4732 提供 `MouseWheel::source`；starfish-we
            // 提供对等 API 后同样可替换本判定。
            let source = cfg_select! {
                target_os = "macos" => {
                    if self
                        .scroll_momentum_monitor
                        .as_ref()
                        .is_some_and(|monitor| monitor.latest_scroll_event_is_momentum())
                    {
                        egui::MouseWheelSource::Momentum
                    } else {
                        // On macOS, only trackpads (and the Magic Mouse) report precise deltas:
                        match unit {
                            egui::MouseWheelUnit::Point => egui::MouseWheelSource::Trackpad,
                            egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => {
                                egui::MouseWheelSource::Wheel
                            }
                        }
                    }
                }
                _ => egui::MouseWheelSource::Unknown,
            };

            self.egui_input.events.push(egui::Event::MouseWheel {
                unit,
                delta,
                phase,
                source,
                modifiers,
            });
        }
    }

    /// 键盘事件的 egui 域主体（剪贴板快捷键折叠、Key/Text 发射；沿用上游抽出的助手）。
    /// starfish-we 无布局逻辑键、无随键文本（文本走 `TextInput`）。
    fn on_key_and_text(
        &mut self,
        pressed: bool,
        logical_key: Option<egui::Key>,
        physical_key: Option<egui::Key>,
        text: Option<&str>,
    ) {
        if let Some(active_key) = logical_key.or(physical_key) {
            if pressed && self.clipboard_shortcuts {
                if is_cut_command(self.modifiers, active_key) {
                    self.egui_input.events.push(egui::Event::Cut);
                    return;
                } else if is_copy_command(self.modifiers, active_key) {
                    self.egui_input.events.push(egui::Event::Copy);
                    return;
                } else if is_paste_command(self.modifiers, active_key) {
                    if let Some(contents) = self.clipboard.get() {
                        let contents = contents.replace("\r\n", "\n");
                        if !contents.is_empty() {
                            self.egui_input.events.push(egui::Event::Paste(contents));
                        }
                    } else if let Some(image) = self.clipboard.get_image() {
                        // No usable text on the clipboard (e.g. an image was copied with
                        // mspaint/Snipping Tool, which never puts a text representation
                        // alongside it) — fall back to an image paste rather than doing
                        // nothing, mirroring `Event::Copy`/`OutputCommand::CopyImage`.
                        self.egui_input
                            .events
                            .push(egui::Event::PasteImage(std::sync::Arc::new(image)));
                    }
                    return;
                }
            }

            self.egui_input.events.push(egui::Event::Key {
                key: active_key,
                physical_key,
                pressed,
                repeat: false, // egui will fill this in for us!
                modifiers: self.modifiers,
            });
        }

        if let Some(text) = text {
            // Make sure there is text, and that it is not control characters
            // (e.g. delete is sent as "\u{f728}" on macOS).
            if !text.is_empty() && text.chars().all(is_printable_char) {
                // On some platforms we get here when the user presses Cmd-C (copy), ctrl-W, etc.
                // We need to ignore these characters that are side-effects of commands.
                // Also make sure the key is pressed (not released). On Linux, text might
                // contain some data even when the key is released.
                let is_cmd =
                    self.modifiers.ctrl || self.modifiers.command || self.modifiers.mac_cmd;
                if pressed && !is_cmd {
                    self.egui_input
                        .events
                        .push(egui::Event::Text(text.to_owned()));
                }
            }
        }
    }
}

// ----------------------------------------------------------------------------

// starfish-we 翻译层：把 [`starfish_we::Event`] 翻译进 [`State`] 的 egui 输入机器。
//
// 内部复用上游 `State` 的全部机制（剪贴板与中键主选择粘贴、剪贴板快捷键折叠、
// `EventResponse` 语义）；winit 特有的布局逻辑键映射、IME 组合窗口、位图光标、
// 多视口在 starfish-we v1 无对应，见各处注记。
//
// 定约（沿用旧版最小桥的约定，升级为支持 HiDPI）：
// - starfish-we 事件坐标 = 物理像素；egui points = 物理像素 ÷ ppp，
//   其中 ppp = `egui_ctx.zoom_factor() × window.dpi_scale()`
//   （未开 `HIGH_PIXEL_DENSITY` 时 dpi_scale = 1，退化为 1:1 域）。
// - starfish-we `KeyCode` 即 W3C code 集，物理键经 [`we_key_to_egui_key`]
//   直通映射；无布局映射的逻辑键，文本一律走 `TextInput`（最终文本通道）。
// - 修饰键随 KeyDown/KeyUp 携带：更新自持副本并广播
//   `egui::Event::ModifiersChanged`（对齐上游 ModifiersChanged 臂）。
//
// 已知缺口（v1）：
// - 指针离开窗口无事件（starfish-we 无 MouseLeave）→ egui hover 会滞留。
// - `PlatformOutput::ime` 不落窗（无 IME 组合 API；TextInput 已是最终文本）。
// - `cursor_image` 位图光标丢弃（无对应 API，同上游即时视口路径）；
//   光标形状图标只映射为可见性开关。
// - 文件拖放无事件（starfish-we v1 无 drag-drop），`dropped_files` /
//   `hovered_files` 恒空。
// - `Touch` 忽略：移动端后端已按 SDL 惯例把主手指合成为鼠标事件，
//   再翻一遍会双份。
// - 多视口/ViewportCommand 不支持：starfish-we 为单窗口 pull 模型。

impl State {
    /// starfish-we 的 `poll_event()` 出一条喂一条；结果见 [`Self::egui_input`]，
    /// 帧首用 [`Self::take_egui_input_we`] 取走。
    pub fn on_we_event(
        &mut self,
        we_window: &starfish_we::Window,
        event: &starfish_we::Event,
    ) -> EventResponse {
        profiling::function_scope!();

        use starfish_we::Event as We;

        #[cfg(target_os = "ios")]
        if matches!(event, We::Resized { .. } | We::FocusGained | We::Resumed) {
            // 安全区（刘海 / 灵动岛）随窗口几何与前后台切换刷新。上游在
            // Resized / ScaleFactorChanged / Focused / Occluded 时刷新；
            // starfish-we 无后两类事件，以这三者近似。
            self.egui_input.safe_area_insets = Some(safe_area::get_safe_area_insets());
        }

        match event {
            We::MouseMoved { x, y } => {
                self.on_we_cursor_moved(we_window, *x, *y);
                EventResponse {
                    repaint: true,
                    consumed: self.egui_ctx.egui_is_using_pointer(),
                }
            }
            We::MouseDown { button } => {
                if let Some(button) = translate_we_mouse_button(*button) {
                    self.on_mouse_button_input(true, button);
                }
                EventResponse {
                    repaint: true,
                    consumed: self.egui_ctx.egui_wants_pointer_input(),
                }
            }
            We::MouseUp { button } => {
                if let Some(button) = translate_we_mouse_button(*button) {
                    self.on_mouse_button_input(false, button);
                }
                EventResponse {
                    repaint: true,
                    consumed: self.egui_ctx.egui_wants_pointer_input(),
                }
            }
            We::MouseWheel { delta_x, delta_y } => {
                // 滚轮齿格（一行/一格），正值向上/向右——Line 单位直通
                self.on_wheel(
                    egui::MouseWheelUnit::Line,
                    egui::vec2(*delta_x as f32, *delta_y as f32),
                    egui::TouchPhase::Move,
                );
                EventResponse {
                    repaint: true,
                    consumed: self.egui_ctx.egui_wants_pointer_input(),
                }
            }
            We::KeyDown { key, modifiers, .. } => {
                let egui_wants_keyboard_input = self.egui_ctx.egui_wants_keyboard_input();
                self.on_we_modifiers(modifiers);
                self.on_we_keyboard_input(*key, true);

                // Tab 常消费（egui 焦点循环），对齐上游 on_window_event
                let consumed = egui_wants_keyboard_input || *key == starfish_we::KeyCode::Tab;
                EventResponse {
                    repaint: true,
                    consumed,
                }
            }
            We::KeyUp { key, modifiers } => {
                let egui_wants_keyboard_input = self.egui_ctx.egui_wants_keyboard_input();
                self.on_we_modifiers(modifiers);
                self.on_we_keyboard_input(*key, false);
                EventResponse {
                    repaint: true,
                    consumed: egui_wants_keyboard_input,
                }
            }
            We::TextInput { text } => {
                self.on_we_text_input(text);
                EventResponse {
                    repaint: true,
                    consumed: self.egui_ctx.egui_wants_keyboard_input(),
                }
            }
            We::FocusGained | We::FocusLost => {
                let focused = if cfg!(target_os = "macos") {
                    // 对齐上游 winit#4371 work-around：以窗口真实焦点为准
                    we_window.has_input_focus()
                } else {
                    matches!(event, We::FocusGained)
                };

                self.egui_input.focused = focused;
                if !focused {
                    // 避免修饰键粘滞（对齐上游 Focused 臂）
                    self.modifiers = egui::Modifiers::default();
                }
                self.egui_input
                    .events
                    .push(egui::Event::WindowFocused(focused));
                EventResponse {
                    repaint: true,
                    consumed: false,
                }
            }
            We::Resized { .. } | We::RedrawRequested | We::CloseRequested | We::Resumed => {
                EventResponse {
                    repaint: true,
                    consumed: false,
                }
            }
            We::Suspended => EventResponse {
                repaint: false,
                consumed: false,
            },
            We::Touch { .. } => {
                // 后端已合成鼠标事件，再翻一遍会双份——见模块文档
                EventResponse {
                    repaint: false,
                    consumed: false,
                }
            }
            _ => {
                // `User(UserEvent)`（宿主注入）与 starfish-we 后续新增事件：与 egui 无关，忽略
                EventResponse {
                    repaint: false,
                    consumed: false,
                }
            }
        }
    }

    /// 帧首：取走累积输入（time/screen_rect/viewport 按 we 窗口现值填充）。
    pub fn take_egui_input_we(&mut self, we_window: &starfish_we::Window) -> egui::RawInput {
        profiling::function_scope!();

        self.egui_input.time = Some(self.start_time.elapsed().as_secs_f64());

        // 最小化时宽高为 0（同上游 Windows 注记），交给 screen_rect 的 >0 判断
        let screen_size_in_pixels = screen_size_in_pixels(we_window);
        let screen_size_in_points =
            screen_size_in_pixels / pixels_per_point(&self.egui_ctx, we_window);

        self.egui_input.screen_rect = (screen_size_in_points.x > 0.0
            && screen_size_in_points.y > 0.0)
            .then(|| egui::Rect::from_min_size(egui::Pos2::ZERO, screen_size_in_points));

        // Tell egui which viewport is now active:
        self.egui_input.viewport_id = self.viewport_id;

        self.egui_input
            .viewports
            .entry(self.viewport_id)
            .or_default()
            .native_pixels_per_point = Some(we_window.dpi_scale());

        self.egui_input.take()
    }

    /// 帧尾：接收 egui 平台输出。剪贴板/链接照常落 crate 内部实现；
    /// 光标只支持可见性开关；IME 与位图光标不落窗（见模块文档）。
    pub fn handle_platform_output_we(
        &mut self,
        we_window: &starfish_we::Window,
        platform_output: egui::PlatformOutput,
    ) {
        profiling::function_scope!();

        let egui::PlatformOutput {
            commands,
            cursor_icon,
            cursor_image,
            events: _,                    // handled elsewhere
            mutable_text_under_cursor: _, // only used in eframe web
            ime: _,                       // starfish-we v1 无 IME 组合窗口 API
            accesskit_update,
            num_completed_passes: _,    // `egui::Context::run` handles this
            request_discard_reasons: _, // `egui::Context::run` handles this
        } = platform_output;

        for command in commands {
            match command {
                egui::OutputCommand::CopyText(text) => {
                    self.clipboard.set_text(text);
                }
                egui::OutputCommand::TextSelectionSettled(text) => {
                    self.clipboard.set_primary_text(text);
                }
                egui::OutputCommand::CopyImage(image) => {
                    self.clipboard.set_image(&image);
                }
                egui::OutputCommand::OpenUrl(open_url) => {
                    open_url_in_browser(&open_url.url);
                }
            }
        }

        // 光标：位图光标丢弃（同上游即时视口路径）；形状图标只映射为可见性
        let _ = cursor_image;
        if self.pointer_pos_in_points.is_some() && self.current_cursor_icon != Some(cursor_icon) {
            self.current_cursor_icon = Some(cursor_icon);
            we_window.set_cursor_visible(cursor_icon != egui::CursorIcon::None);
        }

        // we 路径暂无 accesskit 适配器，更新丢弃
        let _ = accesskit_update;
    }

    fn on_we_modifiers(&mut self, modifiers: &starfish_we::Modifiers) {
        self.modifiers.alt = modifiers.alt;
        self.modifiers.ctrl = modifiers.ctrl;
        self.modifiers.shift = modifiers.shift;
        // 用 `Context::os()` 而非 target_os，对齐上游（web 运行期判 OS）
        let is_mac = self.egui_ctx.os().is_mac();
        self.modifiers.mac_cmd = is_mac && modifiers.meta;
        self.modifiers.command = if is_mac {
            modifiers.meta
        } else {
            modifiers.ctrl
        };

        self.egui_input
            .events
            .push(egui::Event::ModifiersChanged(self.modifiers));
    }

    fn on_we_keyboard_input(&mut self, key: starfish_we::KeyCode, pressed: bool) {
        // KeyCode 即 W3C code：物理键直通；无逻辑键、无随键文本
        //（文本由 TextInput 事件负责，见 `on_we_text_input`）。
        let physical_key = we_key_to_egui_key(key);
        self.on_key_and_text(pressed, None, physical_key, None);
    }

    fn on_we_text_input(&mut self, text: &str) {
        // 命令修饰键按住时不进 Text（避免 Ctrl-C 之类副作用），对齐上游文本段
        let is_cmd = self.modifiers.ctrl || self.modifiers.command || self.modifiers.mac_cmd;
        if is_cmd {
            return;
        }
        for c in text.chars() {
            // 控制字符过滤对齐上游（如 macOS 命名键会送来 \u{f728} 之类）
            if is_printable_char(c) {
                self.egui_input.events.push(egui::Event::Text(c.to_string()));
            }
        }
    }

    fn on_we_cursor_moved(&mut self, we_window: &starfish_we::Window, x: f64, y: f64) {
        let pixels_per_point = pixels_per_point(&self.egui_ctx, we_window);
        let pos_in_points = egui::pos2(x as f32 / pixels_per_point, y as f32 / pixels_per_point);
        self.on_cursor_moved_points(pos_in_points);
    }
}

/// 把系统中文字体挂到 egui 上下文（fork 便利方法，上游无此逻辑）。
///
/// egui 默认字体无 CJK 字形，中文会显示为方块。本方法从系统字体目录探测
/// （Windows：微软雅黑→黑体；macOS：苹方；Linux：Noto Sans CJK），读到第一个
/// 就经 [`egui::Context::set_fonts`] 挂为 Proportional 首选 / Monospace 兜底；
/// 一个都没找到时保持默认字体表并返回 `false`。
///
/// 字体是应用层决策（上游 egui-winit 也刻意不做），本助手只为免抄配方。
pub fn load_system_cjk_fonts(ctx: &egui::Context) -> bool {
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &["/System/Library/Fonts/PingFang.ttc"]
    } else if cfg!(target_os = "linux") {
        &[
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        ]
    } else {
        &["C:/Windows/Fonts/msyh.ttc", "C:/Windows/Fonts/simhei.ttf"]
    };

    let mut fonts = egui::FontDefinitions::default();
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("cjk".to_owned(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "cjk".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("cjk".to_owned());
            ctx.set_fonts(fonts);
            return true;
        }
    }
    false
}

// ── feature `render`：starfish-render 桥（`egui_wgpu::Renderer` 包装）──
//
// 命名约定：[`SfEguiRenderer::render`] 收 starfish-render 的 `RenderPass`
// 包装的可变引用（不转移所有权）——内部把通道生命周期擦成 'static 后绘制，
// 调用方全程不接触原生对象；[`SfEguiRenderer::raw_render`] 收原生
// `wgpu::RenderPass<'static>`（不依赖 starfish-render 的场景）。
// egui-wgpu 0.37 的 `Renderer::render` 硬性要求 'static 通道——包装对象的
// `inner_mut()` 借用给不了（借用随包装对象存活），故在
// [`SfEguiRenderer::render`] 内做一次受控 unsafe 转换（安全契约见其 SAFETY 注释）。
//
// 经 `Deref` 可直接使用 `egui_wgpu::Renderer` 的全部方法
// （`update_texture` / `update_buffers` / `free_texture`…）。
#[cfg(feature = "render")]
pub struct SfEguiRenderer {
    inner: egui_wgpu::Renderer,
}

#[cfg(feature = "render")]
impl SfEguiRenderer {
    /// 包装一个 [`egui_wgpu::Renderer`]。
    pub fn new(inner: egui_wgpu::Renderer) -> Self {
        Self { inner }
    }

    /// 接受 starfish-render 的 `RenderPass` 包装的可变引用直接绘制。
    /// 不转移所有权：通道在函数体内结束（raw drop），调用方此后仍可使用 pass。
    pub fn render(
        &self,
        pass: &mut starfish_render::render_pass::render_pass::RenderPass<'_>,
        paint_jobs: &[egui::epaint::ClippedPrimitive],
        screen_descriptor: &egui_wgpu::ScreenDescriptor,
    ) {
        // SAFETY: 把 `inner_mut()` 借出的 `&mut RenderPass<'a>` 擦成
        // `&'static mut RenderPass<'static>`（同类型仅生命周期不同，位型一致；
        // pass 对象本体未被改动，含 wgpu 内部的 encoder guard）。语义等价于
        // wgpu 官方的 `RenderPass::forget_lifetime`。安全性依赖以下不变式：
        // 1. raw 绝不离开本函数体——不返回、不存储、不据以派生其他长命引用；
        //    函数结束时 drop，通道结束、encoder 锁释放；
        // 2. raw 存活期内不再经由 pass / 所属 encoder 做任何其他操作
        //    （`&mut pass` 的独占借用由编译器保证）；
        // 3. 若未来需要 raw 跨函数存活，必须改回值语义（forget_lifetime 或
        //    消费式签名）并重新论证，不得直接放宽本注释。
        #[expect(unsafe_code)]
        let raw: &'static mut _ = unsafe { std::mem::transmute(pass.inner_mut()) };
        self.inner.render(raw, paint_jobs, screen_descriptor);
    }

    /// 原生 wgpu 通道入口（`egui_wgpu::Renderer::render` 原样转发）。
    pub fn raw_render(
        &self,
        rpass: &mut wgpu::RenderPass<'static>,
        paint_jobs: &[egui::epaint::ClippedPrimitive],
        screen_descriptor: &egui_wgpu::ScreenDescriptor,
    ) {
        self.inner.render(rpass, paint_jobs, screen_descriptor);
    }
}

#[cfg(feature = "render")]
impl std::ops::Deref for SfEguiRenderer {
    type Target = egui_wgpu::Renderer;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[cfg(feature = "render")]
impl std::ops::DerefMut for SfEguiRenderer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/// starfish-we `MouseButton` → egui `PointerButton`
///（`Other(原生序号)` 无对应丢弃；中键的主选择粘贴由
/// [`State::on_mouse_button_input`] 内部处理）。
fn translate_we_mouse_button(button: starfish_we::MouseButton) -> Option<egui::PointerButton> {
    Some(match button {
        starfish_we::MouseButton::Left => egui::PointerButton::Primary,
        starfish_we::MouseButton::Right => egui::PointerButton::Secondary,
        starfish_we::MouseButton::Middle => egui::PointerButton::Middle,
        starfish_we::MouseButton::Back => egui::PointerButton::Extra1,
        starfish_we::MouseButton::Forward => egui::PointerButton::Extra2,
        // `Other(原生序号)` 与后续新增按键（#[non_exhaustive]）：无对应，丢弃
        _ => return None,
    })
}

/// starfish-we `KeyCode` → egui `Key`：starfish-we 即 W3C code 集（与 winit
/// `keyboard::KeyCode` 同名同义），本表合并了上游的 `translate_we_key_code`
/// + `key_from_key_code` 两段直通。注意：winit 的 Win 键叫 `SuperLeft/Right`，
/// starfish-we 叫 `MetaLeft/Right`（W3C 命名），此处对齐。
///
/// starfish-we 无 F13+、浏览器/媒体/launch 物理键、`IntlBackslash` 等
/// winit 扩展键位；`Other(扫描码)` 与后续新增键位（#[non_exhaustive]）暂不映射
///（事件照发，物理键为 None；文本仍由 TextInput 送达）。
fn we_key_to_egui_key(key: starfish_we::KeyCode) -> Option<egui::Key> {
    use egui::Key;
    use starfish_we::KeyCode as K;

    Some(match key {
        K::ArrowDown => Key::ArrowDown,
        K::ArrowLeft => Key::ArrowLeft,
        K::ArrowRight => Key::ArrowRight,
        K::ArrowUp => Key::ArrowUp,

        K::Escape => Key::Escape,
        K::Tab => Key::Tab,
        K::Backspace => Key::Backspace,
        K::Enter | K::NumpadEnter => Key::Enter,

        K::Insert => Key::Insert,
        K::Delete => Key::Delete,
        K::Home => Key::Home,
        K::End => Key::End,
        K::PageUp => Key::PageUp,
        K::PageDown => Key::PageDown,

        // Punctuation
        K::Space => Key::Space,
        K::Comma => Key::Comma,
        K::Period => Key::Period,
        K::Semicolon => Key::Semicolon,
        K::Backslash => Key::Backslash,
        K::Slash | K::NumpadDivide => Key::Slash,
        K::BracketLeft => Key::OpenBracket,
        K::BracketRight => Key::CloseBracket,
        K::Backquote => Key::Backtick,
        K::Quote => Key::Quote,

        K::Minus | K::NumpadSubtract => Key::Minus,
        K::NumpadAdd => Key::Plus,
        K::Equal => Key::Equals,

        K::Digit0 | K::Numpad0 => Key::Num0,
        K::Digit1 | K::Numpad1 => Key::Num1,
        K::Digit2 | K::Numpad2 => Key::Num2,
        K::Digit3 | K::Numpad3 => Key::Num3,
        K::Digit4 | K::Numpad4 => Key::Num4,
        K::Digit5 | K::Numpad5 => Key::Num5,
        K::Digit6 | K::Numpad6 => Key::Num6,
        K::Digit7 | K::Numpad7 => Key::Num7,
        K::Digit8 | K::Numpad8 => Key::Num8,
        K::Digit9 | K::Numpad9 => Key::Num9,

        K::KeyA => Key::A,
        K::KeyB => Key::B,
        K::KeyC => Key::C,
        K::KeyD => Key::D,
        K::KeyE => Key::E,
        K::KeyF => Key::F,
        K::KeyG => Key::G,
        K::KeyH => Key::H,
        K::KeyI => Key::I,
        K::KeyJ => Key::J,
        K::KeyK => Key::K,
        K::KeyL => Key::L,
        K::KeyM => Key::M,
        K::KeyN => Key::N,
        K::KeyO => Key::O,
        K::KeyP => Key::P,
        K::KeyQ => Key::Q,
        K::KeyR => Key::R,
        K::KeyS => Key::S,
        K::KeyT => Key::T,
        K::KeyU => Key::U,
        K::KeyV => Key::V,
        K::KeyW => Key::W,
        K::KeyX => Key::X,
        K::KeyY => Key::Y,
        K::KeyZ => Key::Z,

        K::F1 => Key::F1,
        K::F2 => Key::F2,
        K::F3 => Key::F3,
        K::F4 => Key::F4,
        K::F5 => Key::F5,
        K::F6 => Key::F6,
        K::F7 => Key::F7,
        K::F8 => Key::F8,
        K::F9 => Key::F9,
        K::F10 => Key::F10,
        K::F11 => Key::F11,
        K::F12 => Key::F12,

        // Modifier keys — egui now surfaces them as distinct physical
        // variants so games / capture UIs can bind them independently.
        // The collapsed `Modifiers.shift/ctrl/alt/command` booleans still
        // track just the "any side is pressed" state for shortcut matching.
        K::ShiftLeft => Key::ShiftLeft,
        K::ShiftRight => Key::ShiftRight,
        K::ControlLeft => Key::ControlLeft,
        K::ControlRight => Key::ControlRight,
        K::AltLeft => Key::AltLeft,
        K::AltRight => Key::AltRight,
        K::MetaLeft => Key::SuperLeft,
        K::MetaRight => Key::SuperRight,

        // Lock / System keys:
        K::CapsLock => Key::CapsLock,
        K::NumLock => Key::NumLock,
        K::ScrollLock => Key::ScrollLock,
        K::PrintScreen => Key::PrintScreen,
        K::Pause => Key::Pause,
        K::ContextMenu => Key::Menu,

        _ => {
            return None;
        }
    })
}

// ----------------------------------------------------------------------------

fn open_url_in_browser(_url: &str) {
    #[cfg(feature = "webbrowser")]
    if let Err(err) = webbrowser::open(_url) {
        log::warn!("Failed to open url: {err}");
    }

    #[cfg(not(feature = "webbrowser"))]
    {
        log::warn!("Cannot open url - feature \"links\" not enabled.");
    }
}

/// The window layer hands special keys (backspace, delete, F1, …) to egui as
/// keys, but text channels can still carry private-use/control codepoints.
/// Ignore those. We also ignore '\r', '\n', '\t'.
/// Newlines are handled by the `Key::Enter` event.
fn is_printable_char(chr: char) -> bool {
    let is_in_private_use_area = '\u{e000}' <= chr && chr <= '\u{f8ff}'
        || '\u{f0000}' <= chr && chr <= '\u{ffffd}'
        || '\u{100000}' <= chr && chr <= '\u{10fffd}';

    !is_in_private_use_area && !chr.is_ascii_control()
}

fn is_cut_command(modifiers: egui::Modifiers, keycode: egui::Key) -> bool {
    keycode == egui::Key::Cut
        || (modifiers.command_only() && keycode == egui::Key::X)
        || (cfg!(target_os = "windows") && modifiers.shift && keycode == egui::Key::Delete)
}

fn is_copy_command(modifiers: egui::Modifiers, keycode: egui::Key) -> bool {
    keycode == egui::Key::Copy
        || (modifiers.command_only() && keycode == egui::Key::C)
        || (cfg!(target_os = "windows") && modifiers.ctrl && keycode == egui::Key::Insert)
}

fn is_paste_command(modifiers: egui::Modifiers, keycode: egui::Key) -> bool {
    keycode == egui::Key::Paste
        || (modifiers.command_only() && keycode == egui::Key::V)
        || (cfg!(target_os = "windows") && modifiers.shift && keycode == egui::Key::Insert)
}
