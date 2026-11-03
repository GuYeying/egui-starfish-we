# egui-starfish-we

**starfish-we 拉模型窗口层的 egui 适配层**——egui-winit 0.37.0 vendored
fork（winit 已剥离），为 [starfish](https://github.com/GuYeying/starfish)
生态补上即用型 egui UI 面。

## 与上游的差异

- 原始代码衍生自
  [egui-winit 0.37.0](https://github.com/emilk/egui)（Emil Ernerfeldt and
  egui contributors，MIT OR Apache-2.0）；许可全文见 `LICENSE-MIT` /
  `LICENSE-APACHE`（未改动），修改清单见 `NOTICE`
- **winit 依赖整体剥离**（2026-10-24）：上游以 winit `Window` /
  `WindowEvent` 为词汇的路径（回调式事件循环、多视口命令、IME 组合窗、
  位图光标、accesskit、窗口几何持久化）已移除——本 crate 只认
  starfish-we 的 pull 模型：
  `Window::poll_event()` → `State::on_we_event` → `State::take_egui_input_we`
  → UI → `State::handle_platform_output_we`
- 内部机制沿用上游：剪贴板、快捷键折叠、键盘/指针/滚轮翻译、macOS 滚动
  动量判定；另附 `clipboard` / `safe_area`（刘海屏安全区）等平台便利口

## 用法

```rust
use egui_starfish_we::State;

let mut egui = State::new_we(&window, ...);
// 帧内：事件喂入 → UI 构建 → 输出回灌
```

完整示例：`example/render_test.rs`（`cargo run --example render_test`）。

## 依赖须知

同机路径依赖（设计文档 §3）：本仓库与
[starfish](https://github.com/GuYeying/starfish) 主线共存于同级目录
（`../starfish`），依赖 `starfish-we` / `starfish-render` / `starfish-std`。
克隆本仓库后如路径不符，把 starfish clone 到同级目录即可。

## 生态仓库矩阵

| 仓库 | 角色 |
|---|---|
| [starfish](https://github.com/GuYeying/starfish) | 引擎本体（纯 Rust 跨平台库，零 Python 资产） |
| [pygamest](https://github.com/GuYeying/pygamest) | Python 绑定层（RustPython 内嵌 + 原生 pygame 面） |
| [pygamest-dev](https://github.com/GuYeying/pygamest-dev) | 开发解释器（python.exe 形态，零 Rust 工具链） |
| [pygamest-runtime](https://github.com/GuYeying/pygamest-runtime) | 游戏运行时（玩家单元，入口加密嵌入 + 散文件清单） |
| [egui-starfish-we](https://github.com/GuYeying/egui-starfish-we) | egui × starfish-we 适配（本仓，vendored egui-winit fork） |
| [rapier-starfish-debug](https://github.com/GuYeying/rapier-starfish-debug) | Rapier 物理适配 + 通用 debug 渲染 |

## 协议

MIT OR Apache-2.0 双协议（衍生自 egui-winit 同款双许可；见
LICENSE-MIT / LICENSE-APACHE 与 NOTICE 变更声明）。
