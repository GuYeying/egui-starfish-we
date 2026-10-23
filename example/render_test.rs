//! render_test：starfish-we 窗口 + starfish-render wgpu surface + egui（egui_starfish_we 桥）
//! 三角形与 egui 面板同 pass 上屏；egui 按钮/悬停验证指针与键盘输入。

use std::fs;

use bytemuck::cast_slice;
use egui::viewport::ViewportId;
use egui::Context;
use egui_starfish_we::State;
use starfish_render::render_entry::RenderEntry;
use starfish_render::settings::{GpuSettings, SurfaceSettings};
use starfish_time::Clock;
use starfish_we::{Event, Window};
use wgpu::{Backends, InstanceFlags, MemoryHints, PowerPreference, TextureUsages};

async fn app_body() -> Result<(), Box<dyn std::error::Error>> {
    // 窗口
    let window = Window::builder()
        .title("egui-starfish-we render_test")
        .size((800, 600))
        .build()
        .expect("建窗失败");
    let surface_settings = SurfaceSettings::default()
        .with_usage(TextureUsages::RENDER_ATTACHMENT)
        .with_frame_latency(2);
    let gpu_settings = GpuSettings::default()
        .with_backends(Backends::GL)
        .with_power_preference(PowerPreference::LowPower)
        .with_flags(InstanceFlags::empty())
        .with_memory_hints(MemoryHints::MemoryUsage);
    // 渲染上下文
    let (context, resouce, mut surface) = RenderEntry::async_new(&window, surface_settings, gpu_settings)
        .await
        .expect("RenderContext 初始化失败");

    // egui 输入桥（starfish-we 路径）+ egui 渲染器
    let egui_ctx = Context::default();
    egui_starfish_we::load_system_cjk_fonts(&egui_ctx);

    let mut egui_state = State::new_we(egui_ctx.clone(), ViewportId::ROOT, None, &window);

    let mut egui_renderer =
        egui_starfish_we::SfEguiRenderer::new(egui_wgpu::Renderer::new(
            context.device(),
            surface.color_format(),
            egui_wgpu::RendererOptions::PREDICTABLE,
        ));

    // 1. 纯色三角形着色器（无贴图）
    let shader_source: String = fs::read_to_string("example/triangle.wgsl").unwrap();
    let shader = resouce.shader_module_builder(shader_source)
        .build(Some("triangle_shader"));
    // 2. 三角形顶点数据：pos xyz + color rgb，共3个顶点，无索引
    let tri_verts: &[f32] = &[
        0.0,  0.5, 0.0, 1.0, 0.0, 0.0,
        -0.5, -0.5, 0.0, 0.0, 1.0, 0.0,
        0.5, -0.5, 0.0, 0.0, 0.0, 1.0,
    ];
    // 顶点布局：仅位置+颜色，无UV
    let layout = vec![wgpu::VertexFormat::Float32x3, wgpu::VertexFormat::Float32x3];
    // 构建Mesh：不使用索引缓冲区
    let mesh = resouce.mesh_builder(layout, cast_slice(tri_verts).to_vec())
        .build(Some("tri_vb"), Some("tri_ib"));
    // 3. 材质：不需要纹理、采样器
    let bind_group = resouce.bind_group_builder()
        .build(Some("tri_bind_group"));
    // 4. 2D管线（无深度）
    let pipeline = resouce.render_pipeline_builder_2d(&shader)
        .build(&[&bind_group], &mesh, Some("tri_pipeline"));

    let mut clock = Clock::new();

    // 主循环（SDL 风格 pull：调用方持循环，poll_event 拉事件）
    loop {
        clock.tick(120);
        while let Some(event) = window.poll_event() {
            if matches!(event, Event::CloseRequested) {
                return Ok(());
            }
            // 每条事件喂 egui 桥；consumed 仅信息，v1 不做事件拦截
            let _resp = egui_state.on_we_event(&window, &event);
            match event {
                Event::Resized { width, height } => surface.resize(width, height),
                _ => {}
            }
        }

        // 帧首：取走累积输入；run_ui = begin_pass + root_ui + end_pass（egui 0.37 模式）
        let raw_input = egui_state.take_egui_input_we(&window);
        let mut egui_output = egui_ctx.run_ui(raw_input, |ui| {
            // 半透明面板：透出下层三角形，同框验证两层管线
            egui::CentralPanel::default()
                .frame(egui::Frame::default().fill(egui::Color32::from_rgba_unmultiplied(
                    0, 0, 0, 0,
                )))
                .show(ui, |ui| {
                    ui.heading("egui-starfish-we render_test");
                    if ui.button("Click").clicked() {
                        println!("click");
                    }
                });
        });
        let paint_jobs = egui_ctx.tessellate(egui_output.shapes, egui_output.pixels_per_point);

        // 帧起点：获取交换链纹理并填充 color_attachment；false = 表面未就绪，跳过本帧 GPU 提交
        let frame_ready = surface.begin_frame(wgpu::Color::BLACK, 1.0);

        // 0.37 前置要求：先应用纹理增量，再 update_buffers，最后才能 render
        for (id, deltas) in &egui_output.textures_delta.set {
            for delta in deltas {
                egui_renderer.update_texture(context.device(), context.queue(), *id, delta);
            }
        }
        // ppp 与 tessellate 同源，保证缩放一致
        let (w, h) = window.size();
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [w, h],
            pixels_per_point: egui_output.pixels_per_point,
        };
        let mut egui_encoder = context.device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("egui_upload") });
        let mut cmds = egui_renderer.update_buffers(
            context.device(),
            context.queue(),
            &mut egui_encoder,
            &paint_jobs,
            &screen_descriptor,
        );
        cmds.push(egui_encoder.finish());

        if frame_ready
            && let Some(attachment) = surface.get_current_color_attachment()
        {
            let color_attachment = Some(attachment); // 槽位类型 &Option<ColorAttachment>
            let mut encoder_draw = resouce.create_command_encoder();
            let color_atts = [&color_attachment];
            let mut pass = encoder_draw.begin_render_pass(
                "triangle_pass",
                &color_atts,
                None,
                None,
                None,
                None,
            );
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group);
            pass.set_mesh(&mesh);
            pass.draw(0..mesh.vertex_count(), 0..1);
            //draw ui
            egui_renderer.render(&mut pass, &paint_jobs, &screen_descriptor);

            pass.end();

            cmds.push(encoder_draw.finish());
        }
        surface.submit(cmds);
        surface.present();

        // 释放本帧退场的纹理；再清空 delta（防 TexturesDelta drop panic）
        for id in &egui_output.textures_delta.free {
            egui_renderer.free_texture(id);
        }
        egui_output.textures_delta.clear();

        // 帧尾：光标可见性/剪贴板/链接等平台输出
        egui_state.handle_platform_output_we(&window, egui_output.platform_output);

        starfish_we::next_frame().await;
    }
}

starfish_std::app_entry!(app_body());
