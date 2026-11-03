# pygame-rs / starfish-pygame 渲染架构优化计划（终版 v6）

## 〇、现状结论（已逐项验证）

1. **默认管线本就 2D+3D 一体**：`SpriteVertex { pos: [f32;3], uv, color }`、`render_depth()` 深度会话 + 管线深度变体、`Camera::perspective` 可设——空间渲染设施已在，缺的只是入口。
2. **剔除/混合默认已正确**：sprite 管线走 `RenderPipelineBuilder::new_2d`（builder.rs:184-204）——默认 `CullMode::None + BlendMode::Alpha`（**不剔除，镜像矩阵天然安全**）；3D builder 默认 `CullMode::Back`；setter 全在——细粒度管线控制的接收端已就绪，缺 pygame 侧配置口。
3. **Surface 定位待纯化**：该是"离屏渲染画布 + 空间可放置的纹理对象"双面一体。现况 Screen/Surface/MRT 各持一套 CameraRig（每 Surface 一份纯浪费）；set_camera 三入口保留但**从未绑定 Python、零调用者**。
4. **Material 名不副实但用法正确**："pygame 固定渲染管线的状态材质"（内嵌 sprite shader + 零顶点布局模板 + 变体缓存 HashMap），唯一消费者 batch.rs::draw_stream——改名内部化、不暴露。
5. **三管线同构性**：sprite.wgsl 与 rapier 调试线 shape.wgsl 已同构（`@group(0)@binding(0) = 64B mat4x4 MVP`，调试线带 camera_bind() 共享访问器）；font 的 text.wgsl 同约定。**egui 唯一不同构**：egui-wgpu 0.37 为 crates.io 原版，group(0)=16B Locals、NDC 直写无矩阵。
6. **font/gfxdraw 零改造**：font.render→Surface→blit、gfxdraw 全走 `starfish_pygame::draw::*`——都在 Batch 路径，全局相机化后自动对齐。
7. Surface/Texture 自研直摸 wgpu 是有意例外（base TextureUsage 单选枚举表达不了三位一体 usage）——保留文档化。
8. `pygame.init()` 现为占位；Gpu/Screen 在 set_mode 时构造；resize 已有接线点 `display::handle_resized`（event.rs:187）。

## 一、目标架构定约

- **全局相机 bind 定约（新 ADR 候选）**：`@group(0)@binding(0) { mvp: mat4x4<f32> }`（64B）为所有 starfish 系管线公共第一绑定——sprite/调试线/font 已满足，egui vendor 改造后满足，自定义管线遵守即自动吃全局相机。**set_camera 一处设置、全体生效**。
- **Surface = 离屏画布 + 空间纹理对象**：无相机字段（全局隐式）；`blit(dest)` 屏幕平面贴 / `blit_transform(mat4)` 空间放置（单接口，Mat4 承载一切变换）。
- **L1 starfish-pygame**：全局设施（Gpu：access×2 + 全局唯一 CameraRig + SpriteMaterial + white）+ Screen/Surface + Batch + 重导出门面。
- **L2 pygame-rs**：暴露面两入口（ResourceAccess + 主屏 RenderSurface）+ builder 产物；wgpu 裸对象零出界；不开 features 口；双渲染会话 `with render()` / `with raw_render()`；配置口 `pygame.render.init(args)`。

## 二、阶段 A：层级调整（本次实施）

**A0 pygame.render.init(args)——渲染管线细粒度配置口**
- 愿望槽（thread_local，set_mode 前设置，Gpu/SpriteMaterial 构造时消费一次）。
- v1 参数（安全字符串枚举）：`dim=2|3`（默认 2，覆盖 pygame.init(dim)，后设优先）、`fov/near/far`（dim=3 相机）、`cull="none"|"back"|"front"`（默认 none）、`blend="alpha"|"opaque"|"premultiplied"`（默认 alpha）。
- SpriteMaterial 吸收愿望：cull/blend 进 pipeline build（变体缓存键扩两维；一次定型仅一套变体）。
- 默认值 = 现行为（new_2d 默认）——零回归锚点；镜像矩阵在默认 cull=none 下天然安全，无需 CPU 绕序 hack。

**A1 全局相机（set_camera 能力升级为全局，非移除）**
- Gpu 增全局 CameraRig（一份）+ `CameraState = Auto2d | Auto3d{fov,near,far} | Manual(Camera)`（初值由 A0 定型）。
- **set_camera 三入口保留**：①会话内 `t.set_camera(cam)`（RenderPass::set_camera 保留，转发全局）②全局函数 `render::set_camera(&Camera)` / `reset_camera()` / `camera_mvp()` ③配置口 render.init。**语义 = 全局 override**：设置后所有后续渲染（本帧剩余批次、其它 Surface、后续帧）生效，直到 reset。Rust 侧仅收敛冗余结构体方法 `Surface::set_camera`/`Screen::set_camera`（每面一套的旧形态，零调用者）——内部实现收敛，能力不降反升。
- Python 绑定（首次开放）：`PyCamera`（`Camera.ortho(w,h)` / `Camera.perspective(fov,aspect,near,far)` / `.with_view(mat4)`，view 经已有 PyMat4）+ `t.set_camera(cam)` + `pygame.render.set_camera/reset_camera`。
- target.rs：删三处 camera 字段/构造（:202/:242、:631、:839/:876）；resize 的 set_ortho 钩子删。
- pack 语义（Screen:414/Surface:1045/pack_mrt:668）：Auto2d=ortho(目标自身尺寸)；Auto3d=perspective(fov,目标宽高比)×identity；Manual=用户 mvp。
- `pygame.init(dim=2)`（mod.rs:912）= render.init(dim=) 引擎级简写；dim=3 下 2D 像素坐标被透视解释属预期，文档明示。
- resize 钩子：`display::handle_resized` 追加——Auto 状态下主屏相机矩阵失效重算（pack 现算兜底，保证事件回调里 camera_mvp() 已新）；Manual 不动。
- 效果：N Surface 省 N 份 uniform+bind；font/gfxdraw 自动对齐。

**A2 blit_transform——单接口收拢（Mat4 承载一切变换）**
- **唯一入口** `blit_transform(src, mat4, tint=None)`，无 2D/3D 分裂：2D = xy 仿射（四角 z 恒 0）；3D 空间放置 = 模型矩阵。
- batch.rs：`push_quad_corners` 扩 3D 原语（corners `[[f32;3];4]`）。
- DrawTarget 新增 `blit_transform(&self, src, mat, tint)`：src.flush() → 四角 (0,0,0)(w,0,0)(w,h,0)(0,h,0) 统一 `transform_point3` → push quad；tint 与 blit_tint 相乘。默认 cull=none 下镜像天然安全；用户开剔除后镜像背面消失属物理正确，文档明示。
- 深度联动：`render_depth()` 会话下空间面片与 3D 场景正确遮挡。
- pygame-rs：三会话绑 `blit_transform`（PyMat4 已有）。
- 新 example `blit_transform_demo.py`：①2D 三联（旋转/缩放/镜像 vs transform.rotate 烘焙对比）②`render.init(dim=3)` 模型矩阵空间放置 + rapier 调试线深度遮挡。

**A3 Material→SpriteMaterial**：改名 + 降 pub(crate) + 定位注释改写（含 A0 愿望吸收）；SPRITE_WGSL 保持 pub。

**A4 附件收敛**：删 PassColor/PassDepth，EncodeCtx 直用 starfish ColorAttachment/DepthAttachment（load/store 逐字保持），与 OverlayCtx 同轨。

**A5 Texture/Surface 定位文档化**：三位一体 usage 例外 + "离屏画布 + 空间纹理对象"双面定位。

**验收**：cargo 全绿（starfish + pygame-rs）；桌面回归 surf_probe/pip_test/rapier3d_test/targets MRT；blit_transform_demo 盲验（2D 三联 + 3D 空间遮挡）；build_web.py 一轮 wasm。

## 三、阶段 B：客人相机对齐（egui 优先）

**B1 egui（vendor fork egui-wgpu，硬路首选）**
- vendor egui-wgpu 0.37 进 egui-starfish-we（`vendor/egui-wgpu-0.37/`），pygame-rs Cargo.toml `[patch.crates-io]` 声明。
- 改造三处：①egui.wgsl——Locals 扩 `{ mvp: mat4x4, dithering, predictable }`（保留 frag 字段），`position_from_screen` 改矩阵乘；②renderer.rs UniformBuffer+min_binding_size 同步；③update_buffers——**默认 MVP = 现公式等价矩阵**（`scale(2/w,-2/h,1)·translate(-1,+1)`，零回归锚点）+ 覆盖通道。
- SfEguiRenderer 增 `set_camera_mvp(Mat4)`；pygame_gui runtime draw_ui 时传全局 mvp（离屏面按面尺寸、ppp=1.0 不变）。
- 验收：egui_test/pip_test 默认矩阵像素级回归 + set_camera 下 egui 跟随演示。
- 备选（遇阻）：callback_resources paint callback（能力打折）。

**B2 rapier 调试线接全局相机**：零管线改造——排程时读全局 camera_mvp() write_buffer 到调试线 camera_buf；set_camera_ppm/set_camera_lookat 显式优先。

**B3 font/gfxdraw**：零改造，回归确认。

## 四、阶段 C：重导出 + Python 包装（收缩暴露面）

- C1 门面：`pub use starfish_render::{RenderResourceAccess, RenderSurface, ShaderModule, Mesh, MeshBuilder, BindGroup, BindGroupBuilder, UniformBuffer, RenderPipeline, RenderPipelineBuilder, SamplerDescriptor, ColorAttachment, DepthAttachment, …}`；冲突对（Texture/RenderPass）走 `pub mod guest`。
- C2 pygame-rs 包装：PyResourceAccess（builder 族，收参安全枚举——与 A0 枚举设施同源）+ PyRenderSurface（get_color/depth_attachment → Py 包装）+ 产物 Py 类（PyShaderModule/PyMesh/PyBindGroup/PyUniformBuffer/PyRenderPipeline/PySampler），名字空间 pygame.render.*。
- C3 不暴露 features/GpuSettings/Device/Queue；自定义管线遵循全局相机 bind 定约即自动吃全局相机。

## 五、阶段 D：`with raw_render()` 直绘会话

- `with screen.raw_render(depth=False) as p:` / `with surface.raw_render() as p:` 独立入口 PyRawPass：set_pipeline/set_bind_group/set_mesh+draw_mesh/set_scissor/set_viewport/end。
- 实现：命令记录，__exit__ 在目标提交时点经 OverlayCtx::with_pass 回放；attachment 引擎自取；深度定约（Clear 1.0、恒在内容之上）不回退；PyRenderSurface.get_attachment 留逃生口。
- example：自定义 WGSL 渐变 quad + 自建管线旋转贴图。

## 六、风险

- set_camera 语义变化（每目标→全局）：零现存调用者，无迁移成本；t.set_camera 会话形态不变。
- A0 愿望消费一次定型：set_mode 后再 render.init 的定约（v1 = 报错或下次 set_mode 生效）实施时定并文档化。
- A4 附件 load 值逐字保持（画中画底色回归敏感）。
- egui vendor 版本锁：Cargo.lock 全链更新，web/android 产物重打包验证。
- dim=3 下 2D 绘制语义漂移属预期。
- 批次 51 深度定约（with_pass 恒 Clear 1.0）全程不回退。

**实施批次**：本次先做阶段 A（A0 配置口 + A1 全局相机/PyCamera + A2 blit_transform + A3 改名 + A4 附件收敛 + A5 文档化）；B/C/D 为后续批次（方案已定，逐批审批推进）。