use protocol::{AdapterInfo, CanvasSize, PickResult, Stats};
use wasm_bindgen::JsValue;
use web_sys::OffscreenCanvas;
use web_time::Instant;
use wgpu::InstanceDescriptor;
use wgpu::util::DeviceExt;

pub struct WgpuApp {
    gpu: Gpu,
    depth_view: wgpu::TextureView,
    scene: Scene,
    controls: Controls,
    camera: OrbitCamera,
    stats: FrameStats,
    last_frame: Instant,
}

impl WgpuApp {
    pub async fn create(canvas: OffscreenCanvas, size: CanvasSize) -> WgpuApp {
        let width = (size.width as u32).max(1);
        let height = (size.height as u32).max(1);

        let gpu = Gpu::new_async(canvas, width, height).await;
        let depth_view = gpu.create_depth_texture(width, height);
        let scene = Scene::new(&gpu.device, gpu.surface_format);

        WgpuApp {
            gpu,
            depth_view,
            scene,
            controls: Controls::default(),
            camera: OrbitCamera::default(),
            stats: FrameStats::default(),
            last_frame: Instant::now(),
        }
    }

    pub fn update(&mut self) {
        let now = Instant::now();
        let delta_time = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;

        self.stats.frames += 1;
        self.stats.accumulator += delta_time;
        self.stats.window_frames += 1;
        if self.stats.accumulator >= 0.25 {
            self.stats.fps = self.stats.window_frames as f32 / self.stats.accumulator;
            self.stats.accumulator = 0.0;
            self.stats.window_frames = 0;
        }

        self.scene.update(
            &self.gpu.queue,
            self.gpu.aspect_ratio(),
            delta_time,
            &self.controls,
            &self.camera,
        );
        self.render();
    }

    pub fn resize(&mut self, size: CanvasSize) {
        let width = (size.width as u32).max(1);
        let height = (size.height as u32).max(1);
        self.gpu.resize(width, height);
        self.depth_view = self.gpu.create_depth_texture(width, height);
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.controls.speed = speed;
    }

    pub fn set_color(&mut self, red: f32, green: f32, blue: f32) {
        self.controls.tint = [red, green, blue, 1.0];
    }

    pub fn orbit(&mut self, delta_yaw: f32, delta_pitch: f32) {
        self.camera.yaw += delta_yaw * ORBIT_SENSITIVITY;
        self.camera.pitch =
            (self.camera.pitch + delta_pitch * ORBIT_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    pub fn zoom(&mut self, amount: f32) {
        self.camera.distance = (self.camera.distance + amount * ZOOM_SENSITIVITY).clamp(1.5, 20.0);
    }

    pub fn stats(&self) -> Stats {
        Stats {
            frames: self.stats.frames as f64,
            fps: self.stats.fps,
        }
    }

    pub fn adapter_info(&self) -> AdapterInfo {
        AdapterInfo {
            adapter: self.gpu.adapter_name.clone(),
            backend: self.gpu.backend.clone(),
        }
    }

    pub fn pick(&mut self, x: f32, y: f32) -> Option<PickResult> {
        self.scene.pick(x, y)
    }

    pub fn context(&self) -> String {
        let global = js_sys::global();
        js_sys::Reflect::get(&global, &JsValue::from_str("constructor"))
            .ok()
            .and_then(|constructor| {
                js_sys::Reflect::get(&constructor, &JsValue::from_str("name")).ok()
            })
            .and_then(|name| name.as_string())
            .unwrap_or_else(|| "unknown".to_string())
    }

    fn render(&mut self) {
        let surface_texture = match self.gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            _ => {
                self.gpu
                    .surface
                    .configure(&self.gpu.device, &self.gpu.surface_config);
                return;
            }
        };

        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor {
                label: None,
                aspect: wgpu::TextureAspect::default(),
                format: Some(self.gpu.surface_format),
                dimension: None,
                base_mip_level: 0,
                mip_level_count: None,
                base_array_layer: 0,
                array_layer_count: None,
                usage: None,
            });

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 43.0 / 255.0,
                            g: 44.0 / 255.0,
                            b: 47.0 / 255.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.scene.render(&mut render_pass);
        }

        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        surface_texture.present();
    }
}

struct Gpu {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    surface_format: wgpu::TextureFormat,
    adapter_name: String,
    backend: String,
}

impl Gpu {
    fn aspect_ratio(&self) -> f32 {
        self.surface_config.width as f32 / self.surface_config.height.max(1) as f32
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
    }

    fn create_depth_texture(&self, width: u32, height: u32) -> wgpu::TextureView {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        texture.create_view(&wgpu::TextureViewDescriptor::default())
    }

    async fn new_async(canvas: OffscreenCanvas, width: u32, height: u32) -> Self {
        let instance =
            wgpu::Instance::new(InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::OffscreenCanvas(canvas))
            .expect("failed to create surface from offscreen canvas");

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("failed to request adapter");

        let adapter_info = adapter.get_info();

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("device"),
                memory_hints: wgpu::MemoryHints::default(),
                required_features: wgpu::Features::default(),
                required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                trace: wgpu::Trace::Off,
            })
            .await
            .expect("failed to request device");

        let surface_capabilities = surface.get_capabilities(&adapter);
        let surface_format = surface_capabilities
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .unwrap_or(surface_capabilities.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width,
            height,
            present_mode: surface_capabilities.present_modes[0],
            alpha_mode: surface_capabilities.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        Self {
            surface,
            device,
            queue,
            surface_config,
            surface_format,
            adapter_name: adapter_info.name,
            backend: format!("{:?}", adapter_info.backend),
        }
    }
}

struct Scene {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform: UniformBinding,
    pipeline: wgpu::RenderPipeline,
    spin: f32,
    view_proj: nalgebra_glm::Mat4,
    model: nalgebra_glm::Mat4,
    hit_point: [f32; 4],
}

impl Scene {
    fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let (vertices, indices) = build_cube(0.8);

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertex buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("index buffer"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let uniform = UniformBinding::new(device);
        let pipeline = create_pipeline(device, surface_format, &uniform);

        Self {
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
            uniform,
            pipeline,
            spin: 0.0,
            view_proj: nalgebra_glm::Mat4::identity(),
            model: nalgebra_glm::Mat4::identity(),
            hit_point: [0.0, 0.0, 0.0, 0.0],
        }
    }

    fn update(
        &mut self,
        queue: &wgpu::Queue,
        aspect_ratio: f32,
        delta_time: f32,
        controls: &Controls,
        camera: &OrbitCamera,
    ) {
        self.spin += controls.speed * delta_time;

        let projection =
            nalgebra_glm::perspective_lh_zo(aspect_ratio, 60_f32.to_radians(), 0.1, 100.0);
        let view = nalgebra_glm::look_at_lh(
            &camera.eye(),
            &nalgebra_glm::vec3(0.0, 0.0, 0.0),
            &nalgebra_glm::vec3(0.0, 1.0, 0.0),
        );
        let model = nalgebra_glm::rotation(self.spin, &nalgebra_glm::vec3(0.0, 1.0, 0.0))
            * nalgebra_glm::rotation(self.spin * 0.4, &nalgebra_glm::vec3(1.0, 0.0, 0.0));

        let view_proj = projection * view;
        self.view_proj = view_proj;
        self.model = model;

        self.uniform.update_buffer(
            queue,
            UniformBuffer {
                mvp: view_proj * model,
                model,
                tint: controls.tint,
                hit_point: self.hit_point,
            },
        );
    }

    fn pick(&mut self, ndc_x: f32, ndc_y: f32) -> Option<PickResult> {
        let inverse_view_proj = nalgebra_glm::inverse(&self.view_proj);
        let near = inverse_view_proj * nalgebra_glm::vec4(ndc_x, ndc_y, 0.0, 1.0);
        let far = inverse_view_proj * nalgebra_glm::vec4(ndc_x, ndc_y, 1.0, 1.0);
        let near = nalgebra_glm::vec3(near.x / near.w, near.y / near.w, near.z / near.w);
        let far = nalgebra_glm::vec3(far.x / far.w, far.y / far.w, far.z / far.w);

        let inverse_model = nalgebra_glm::inverse(&self.model);
        let origin = inverse_model * nalgebra_glm::vec4(near.x, near.y, near.z, 1.0);
        let origin = nalgebra_glm::vec3(origin.x, origin.y, origin.z);
        let direction = far - near;
        let direction =
            inverse_model * nalgebra_glm::vec4(direction.x, direction.y, direction.z, 0.0);
        let direction =
            nalgebra_glm::normalize(&nalgebra_glm::vec3(direction.x, direction.y, direction.z));

        let (point, face) = ray_cube_hit(origin, direction, 0.8)?;
        self.hit_point = [point.x, point.y, point.z, 1.0];
        Some(PickResult {
            face: face.to_string(),
            x: point.x,
            y: point.y,
            z: point.z,
        })
    }

    fn render<'pass>(&'pass self, render_pass: &mut wgpu::RenderPass<'pass>) {
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.uniform.bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        render_pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        render_pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}

fn create_pipeline(
    device: &wgpu::Device,
    surface_format: wgpu::TextureFormat,
    uniform: &UniformBinding,
) -> wgpu::RenderPipeline {
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("cube shader"),
        source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(SHADER_SOURCE)),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("pipeline layout"),
        bind_group_layouts: &[Some(&uniform.bind_group_layout)],
        immediate_size: 0,
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("cube pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader_module,
            entry_point: Some("vertex_main"),
            buffers: &[Vertex::layout()],
            compilation_options: Default::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
            unclipped_depth: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: 1,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader_module,
            entry_point: Some("fragment_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 4],
    normal: [f32; 4],
}

impl Vertex {
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRIBUTES: [wgpu::VertexAttribute; 2] =
            wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRIBUTES,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct UniformBuffer {
    mvp: nalgebra_glm::Mat4,
    model: nalgebra_glm::Mat4,
    tint: [f32; 4],
    hit_point: [f32; 4],
}

impl Default for UniformBuffer {
    fn default() -> Self {
        Self {
            mvp: nalgebra_glm::Mat4::identity(),
            model: nalgebra_glm::Mat4::identity(),
            tint: [0.3, 0.5, 0.9, 1.0],
            hit_point: [0.0, 0.0, 0.0, 0.0],
        }
    }
}

struct UniformBinding {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    bind_group_layout: wgpu::BindGroupLayout,
}

impl UniformBinding {
    fn new(device: &wgpu::Device) -> Self {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("uniform buffer"),
            contents: bytemuck::cast_slice(&[UniformBuffer::default()]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("uniform layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniform bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });

        Self {
            buffer,
            bind_group,
            bind_group_layout,
        }
    }

    fn update_buffer(&self, queue: &wgpu::Queue, uniform: UniformBuffer) {
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&[uniform]));
    }
}

const ORBIT_SENSITIVITY: f32 = 0.005;
const ZOOM_SENSITIVITY: f32 = 0.01;
const PITCH_LIMIT: f32 = 1.5;

struct OrbitCamera {
    yaw: f32,
    pitch: f32,
    distance: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: 0.0,
            distance: 3.0,
        }
    }
}

impl OrbitCamera {
    fn eye(&self) -> nalgebra_glm::Vec3 {
        nalgebra_glm::vec3(
            self.distance * self.pitch.cos() * self.yaw.sin(),
            self.distance * self.pitch.sin(),
            self.distance * self.pitch.cos() * self.yaw.cos(),
        )
    }
}

struct Controls {
    speed: f32,
    tint: [f32; 4],
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            speed: 1.0,
            tint: [0.3, 0.5, 0.9, 1.0],
        }
    }
}

#[derive(Default)]
struct FrameStats {
    frames: u64,
    fps: f32,
    accumulator: f32,
    window_frames: u32,
}

fn ray_cube_hit(
    origin: nalgebra_glm::Vec3,
    direction: nalgebra_glm::Vec3,
    half: f32,
) -> Option<(nalgebra_glm::Vec3, &'static str)> {
    let mut t_min = f32::NEG_INFINITY;
    let mut t_max = f32::INFINITY;
    let mut axis = 0;
    let mut sign = -1.0;
    for index in 0..3 {
        let o = origin[index];
        let d = direction[index];
        if d.abs() < 1e-6 {
            if o < -half || o > half {
                return None;
            }
            continue;
        }
        let inverse = 1.0 / d;
        let mut t1 = (-half - o) * inverse;
        let mut t2 = (half - o) * inverse;
        let mut entry_sign = -1.0;
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
            entry_sign = 1.0;
        }
        if t1 > t_min {
            t_min = t1;
            axis = index;
            sign = entry_sign;
        }
        if t2 < t_max {
            t_max = t2;
        }
        if t_min > t_max {
            return None;
        }
    }
    if t_max < 0.0 {
        return None;
    }
    let t = if t_min >= 0.0 { t_min } else { t_max };
    let point = origin + direction * t;
    let face = match (axis, sign > 0.0) {
        (0, true) => "+X",
        (0, false) => "-X",
        (1, true) => "+Y",
        (1, false) => "-Y",
        (2, true) => "+Z",
        _ => "-Z",
    };
    Some((point, face))
}

fn build_cube(half: f32) -> (Vec<Vertex>, Vec<u32>) {
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        (
            [1.0, 0.0, 0.0],
            [
                [1.0, -1.0, -1.0],
                [1.0, -1.0, 1.0],
                [1.0, 1.0, 1.0],
                [1.0, 1.0, -1.0],
            ],
        ),
        (
            [-1.0, 0.0, 0.0],
            [
                [-1.0, -1.0, 1.0],
                [-1.0, -1.0, -1.0],
                [-1.0, 1.0, -1.0],
                [-1.0, 1.0, 1.0],
            ],
        ),
        (
            [0.0, 1.0, 0.0],
            [
                [-1.0, 1.0, -1.0],
                [1.0, 1.0, -1.0],
                [1.0, 1.0, 1.0],
                [-1.0, 1.0, 1.0],
            ],
        ),
        (
            [0.0, -1.0, 0.0],
            [
                [-1.0, -1.0, 1.0],
                [1.0, -1.0, 1.0],
                [1.0, -1.0, -1.0],
                [-1.0, -1.0, -1.0],
            ],
        ),
        (
            [0.0, 0.0, 1.0],
            [
                [-1.0, -1.0, 1.0],
                [1.0, -1.0, 1.0],
                [1.0, 1.0, 1.0],
                [-1.0, 1.0, 1.0],
            ],
        ),
        (
            [0.0, 0.0, -1.0],
            [
                [1.0, -1.0, -1.0],
                [-1.0, -1.0, -1.0],
                [-1.0, 1.0, -1.0],
                [1.0, 1.0, -1.0],
            ],
        ),
    ];

    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (normal, corners) in faces {
        let base = vertices.len() as u32;
        for corner in corners {
            vertices.push(Vertex {
                position: [corner[0] * half, corner[1] * half, corner[2] * half, 1.0],
                normal: [normal[0], normal[1], normal[2], 0.0],
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    (vertices, indices)
}

const SHADER_SOURCE: &str = "
struct Uniform {
    mvp: mat4x4<f32>,
    model: mat4x4<f32>,
    tint: vec4<f32>,
    hit_point: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> ubo: Uniform;

struct VertexInput {
    @location(0) position: vec4<f32>,
    @location(1) normal: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) local_position: vec3<f32>,
};

@vertex
fn vertex_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.clip_position = ubo.mvp * input.position;
    output.world_normal = (ubo.model * vec4<f32>(input.normal.xyz, 0.0)).xyz;
    output.local_position = input.position.xyz;
    return output;
}

fn srgb_to_linear(color: vec3<f32>) -> vec3<f32> {
    let cutoff = color <= vec3<f32>(0.04045);
    let lower = color / 12.92;
    let higher = pow((color + 0.055) / 1.055, vec3<f32>(2.4));
    return select(higher, lower, cutoff);
}

fn linear_to_srgb(color: vec3<f32>) -> vec3<f32> {
    let cutoff = color <= vec3<f32>(0.0031308);
    let lower = color * 12.92;
    let higher = 1.055 * pow(color, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(higher, lower, cutoff);
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let light_direction = normalize(vec3<f32>(0.4, 0.8, 0.6));
    let diffuse = max(dot(normalize(input.world_normal), light_direction), 0.0);
    let shade = 0.25 + 0.75 * diffuse;
    var color = srgb_to_linear(ubo.tint.rgb) * shade;

    if (ubo.hit_point.w > 0.5) {
        let marker = srgb_to_linear(vec3<f32>(1.0, 0.5, 0.1));
        let edge = smoothstep(0.06, 0.12, distance(input.local_position, ubo.hit_point.xyz));
        color = mix(marker, color, edge);
    }

    return vec4<f32>(linear_to_srgb(color), 1.0);
}
";
