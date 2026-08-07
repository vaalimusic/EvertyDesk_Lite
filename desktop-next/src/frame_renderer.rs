//! Direct WGPU presentation for decoded video frames.
//!
//! Replaces the `pixels` crate as the viewer's presentation backend. `pixels`
//! already uploaded its CPU buffer straight into a `wgpu::Texture`, so this
//! isn't a performance rewrite of today's frame path — the win is dropping a
//! second, differently-versioned `wgpu` from the dependency graph and owning
//! the render pipeline directly, which is what unlocks GPU-side YUV textures
//! later without going through a third-party abstraction that hides its
//! `wgpu::Surface` (pixels keeps it private, so a caller can never plug in a
//! custom presentation step). The RGBA-in/RGBA-texture-out contract and the
//! shared core decoder pipeline are unchanged.
//!
//! The scaling shaders and the coordinate-mapping math below are adapted
//! from the `pixels` crate (MIT/Apache-2.0, <https://github.com/parasyte/pixels>)
//! to keep the visible behavior — fill vs. pixel-perfect scaling, letterbox
//! centering, mouse-to-frame mapping — bit-for-bit identical to what shipped
//! before.

use std::sync::Arc;

use winit::window::Window;

/// Matches `pixels::ScalingMode`; kept as our own type so this module has no
/// dependency on the `pixels` crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalingMode {
    /// Preserve aspect ratio, scale to the nearest integer multiple.
    PixelPerfect,
    /// Preserve aspect ratio, scale to fill the window (may be fractional).
    Fill,
}

#[derive(Debug)]
pub enum FrameRendererError {
    NoAdapter,
    NoDevice(wgpu::RequestDeviceError),
    CreateSurface(wgpu::CreateSurfaceError),
    InvalidSize { width: u32, height: u32 },
    Present(String),
}

impl std::fmt::Display for FrameRendererError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAdapter => write!(f, "no suitable wgpu adapter found"),
            Self::NoDevice(error) => write!(f, "no wgpu device: {error}"),
            Self::CreateSurface(error) => write!(f, "unable to create surface: {error}"),
            Self::InvalidSize { width, height } => {
                write!(f, "invalid texture size {width}x{height}")
            }
            Self::Present(error) => write!(f, "surface error: {error}"),
        }
    }
}

impl std::error::Error for FrameRendererError {}

/// Owns the swapchain surface, the RGBA source texture, and the scaling
/// blit pipeline used to present it. Call [`Self::frame_mut`] to obtain a
/// scratch CPU buffer, write RGBA8 bytes (plus any CPU-composited overlay)
/// into it, then call [`Self::render`] to upload and present.
pub struct FrameRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter: wgpu::Adapter,
    surface: wgpu::Surface<'static>,
    surface_format: wgpu::TextureFormat,
    surface_size: (u32, u32),

    texture: wgpu::Texture,
    texture_size: (u32, u32),
    scratch: Vec<u8>,

    scaling_mode: ScalingMode,
    pipeline_nearest: wgpu::RenderPipeline,
    pipeline_fill: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group_nearest: wgpu::BindGroup,
    bind_group_linear: wgpu::BindGroup,
    sampler_nearest: wgpu::Sampler,
    sampler_linear: wgpu::Sampler,
    vertex_buffer: wgpu::Buffer,
    uniform_buffer: wgpu::Buffer,
    clip_rect: (u32, u32, u32, u32),
}

const TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const BYTES_PER_PIXEL: u32 = 4;

impl FrameRenderer {
    pub fn new(
        window: Arc<Window>,
        texture_width: u32,
        texture_height: u32,
    ) -> Result<Self, FrameRendererError> {
        let size = window.inner_size();
        let surface_size = (size.width.max(1), size.height.max(1));

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::from_env().unwrap_or_else(wgpu::Backends::all),
            ..wgpu::InstanceDescriptor::new_without_display_handle().with_env()
        });
        let surface = instance
            .create_surface(window)
            .map_err(FrameRendererError::CreateSurface)?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::from_env().unwrap_or_default(),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .map_err(|_| FrameRendererError::NoAdapter)?;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .map_err(FrameRendererError::NoDevice)?;

        let capabilities = surface.get_capabilities(&adapter);
        let surface_format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(wgpu::TextureFormat::Bgra8UnormSrgb);

        surface.configure(
            &device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: surface_format,
                width: surface_size.0,
                height: surface_size.1,
                present_mode: wgpu::PresentMode::AutoVsync,
                desired_maximum_frame_latency: 2,
                alpha_mode: capabilities
                    .alpha_modes
                    .first()
                    .copied()
                    .unwrap_or(wgpu::CompositeAlphaMode::Auto),
                view_formats: vec![],
            },
        );

        check_texture_size(&device, texture_width, texture_height)?;
        let texture = create_texture(&device, texture_width, texture_height);
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let shader_nearest = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("evertydesk_scale_nearest"),
            source: wgpu::ShaderSource::Wgsl(SCALE_NEAREST_WGSL.into()),
        });
        let shader_fill = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("evertydesk_scale_fill"),
            source: wgpu::ShaderSource::Wgsl(SCALE_FILL_WGSL.into()),
        });

        let sampler_nearest = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("evertydesk_sampler_nearest"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let sampler_linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("evertydesk_sampler_linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("evertydesk_scale_vertex_buffer"),
            size: (VERTEX_DATA.len() * 8) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&vertex_buffer, 0, vertex_bytes());

        let scaling_mode = ScalingMode::PixelPerfect;
        let uniform_bytes = uniform_buffer_bytes(
            (texture_width as f32, texture_height as f32),
            (surface_size.0 as f32, surface_size.1 as f32),
            scaling_mode,
        );
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("evertydesk_scale_uniform_buffer"),
            size: uniform_bytes.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buffer, 0, &uniform_bytes);

        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("evertydesk_scale_bind_group_layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            multisampled: false,
                            view_dimension: wgpu::TextureViewDimension::D2,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(uniform_bytes.len() as u64),
                        },
                        count: None,
                    },
                ],
            });

        let bind_group_nearest = create_bind_group(
            &device,
            &bind_group_layout,
            &texture_view,
            &sampler_nearest,
            &uniform_buffer,
            "evertydesk_bind_group_nearest",
        );
        let bind_group_linear = create_bind_group(
            &device,
            &bind_group_layout,
            &texture_view,
            &sampler_linear,
            &uniform_buffer,
            "evertydesk_bind_group_linear",
        );

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("evertydesk_scale_pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline_nearest = create_pipeline(
            &device,
            &pipeline_layout,
            &shader_nearest,
            surface_format,
            "evertydesk_pipeline_nearest",
        );
        let pipeline_fill = create_pipeline(
            &device,
            &pipeline_layout,
            &shader_fill,
            surface_format,
            "evertydesk_pipeline_fill",
        );

        let clip_rect = clip_rect_for(
            (texture_width as f32, texture_height as f32),
            (surface_size.0 as f32, surface_size.1 as f32),
            scaling_mode,
        );

        let scratch_len = (texture_width as usize)
            .saturating_mul(texture_height as usize)
            .saturating_mul(BYTES_PER_PIXEL as usize);

        Ok(Self {
            device,
            queue,
            adapter,
            surface,
            surface_format,
            surface_size,
            texture,
            texture_size: (texture_width, texture_height),
            scratch: vec![0; scratch_len],
            scaling_mode,
            pipeline_nearest,
            pipeline_fill,
            bind_group_layout,
            bind_group_nearest,
            bind_group_linear,
            sampler_nearest,
            sampler_linear,
            vertex_buffer,
            uniform_buffer,
            clip_rect,
        })
    }

    /// Mutable scratch buffer for the next frame's RGBA8 bytes (plus any
    /// CPU-composited overlay). Must be fully repopulated before [`Self::render`]
    /// — unlike the old `pixels` buffer, resizing does not preserve contents.
    pub fn frame_mut(&mut self) -> &mut [u8] {
        &mut self.scratch
    }

    pub fn frame(&self) -> &[u8] {
        &self.scratch
    }

    pub fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    pub fn set_scaling_mode(&mut self, scaling_mode: ScalingMode) {
        self.scaling_mode = scaling_mode;
        self.recompute_scaling();
    }

    /// Resize the source (video) texture. Does not touch the surface.
    pub fn resize_buffer(&mut self, width: u32, height: u32) -> Result<(), FrameRendererError> {
        check_texture_size(&self.device, width, height)?;

        let texture = create_texture(&self.device, width, height);
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.bind_group_nearest = create_bind_group(
            &self.device,
            &self.bind_group_layout,
            &texture_view,
            &self.sampler_nearest,
            &self.uniform_buffer,
            "evertydesk_bind_group_nearest",
        );
        self.bind_group_linear = create_bind_group(
            &self.device,
            &self.bind_group_layout,
            &texture_view,
            &self.sampler_linear,
            &self.uniform_buffer,
            "evertydesk_bind_group_linear",
        );
        self.texture = texture;
        self.texture_size = (width, height);

        let scratch_len = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(BYTES_PER_PIXEL as usize);
        self.scratch = vec![0; scratch_len];

        self.recompute_scaling();
        Ok(())
    }

    /// Resize the swapchain surface. Does not touch the source texture.
    pub fn resize_surface(&mut self, width: u32, height: u32) -> Result<(), FrameRendererError> {
        if width == 0 || height == 0 {
            return Err(FrameRendererError::InvalidSize { width, height });
        }
        self.surface_size = (width, height);
        self.reconfigure_surface();
        self.recompute_scaling();
        Ok(())
    }

    fn reconfigure_surface(&self) {
        self.surface.configure(
            &self.device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: self.surface_format,
                width: self.surface_size.0,
                height: self.surface_size.1,
                present_mode: wgpu::PresentMode::AutoVsync,
                desired_maximum_frame_latency: 2,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: vec![],
            },
        );
    }

    fn recompute_scaling(&mut self) {
        let texture_size = (self.texture_size.0 as f32, self.texture_size.1 as f32);
        let surface_size = (self.surface_size.0 as f32, self.surface_size.1 as f32);
        let uniform_bytes = uniform_buffer_bytes(texture_size, surface_size, self.scaling_mode);
        self.queue.write_buffer(&self.uniform_buffer, 0, &uniform_bytes);
        self.clip_rect = clip_rect_for(texture_size, surface_size, self.scaling_mode);
    }

    /// Upload the scratch buffer to the GPU texture and present it, scaled
    /// according to the current [`ScalingMode`].
    pub fn render(&mut self) -> Result<(), FrameRendererError> {
        let expected_len = (self.texture_size.0 as usize)
            .saturating_mul(self.texture_size.1 as usize)
            .saturating_mul(BYTES_PER_PIXEL as usize);
        if self.scratch.len() != expected_len {
            // Nothing valid to present yet (e.g. resize just happened).
            return Ok(());
        }

        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.scratch,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.texture_size.0 * BYTES_PER_PIXEL),
                rows_per_image: Some(self.texture_size.1),
            },
            wgpu::Extent3d {
                width: self.texture_size.0,
                height: self.texture_size.1,
                depth_or_array_layers: 1,
            },
        );

        let surface_texture = loop {
            match self.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(surface_texture) => break surface_texture,
                wgpu::CurrentSurfaceTexture::Suboptimal(surface_texture) => {
                    drop(surface_texture);
                    self.reconfigure_surface();
                }
                wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                    self.reconfigure_surface();
                }
                wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Timeout => {
                    return Ok(());
                }
                wgpu::CurrentSurfaceTexture::Validation => {
                    return Err(FrameRendererError::Present(
                        "surface validation error".to_owned(),
                    ));
                }
            }
        };

        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("evertydesk_frame_encoder"),
            });
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("evertydesk_scale_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let (pipeline, bind_group) = match self.scaling_mode {
                ScalingMode::PixelPerfect => (&self.pipeline_nearest, &self.bind_group_nearest),
                ScalingMode::Fill => (&self.pipeline_fill, &self.bind_group_linear),
            };
            rpass.set_pipeline(pipeline);
            rpass.set_bind_group(0, bind_group, &[]);
            rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            rpass.set_scissor_rect(
                self.clip_rect.0,
                self.clip_rect.1,
                self.clip_rect.2,
                self.clip_rect.3,
            );
            rpass.draw(0..3, 0..1);
        }
        self.queue.submit(Some(encoder.finish()));
        surface_texture.present();
        Ok(())
    }

    /// Map a physical window position to a pixel position in the source
    /// texture. Mirrors `pixels::Pixels::window_pos_to_pixel`.
    pub fn window_pos_to_pixel(
        &self,
        physical_position: (f32, f32),
    ) -> Result<(usize, usize), (isize, isize)> {
        let (screen_w, screen_h) = (self.surface_size.0 as f32, self.surface_size.1 as f32);
        let (tex_w, tex_h) = (self.texture_size.0 as f32, self.texture_size.1 as f32);
        let scale = scale_factor((tex_w, tex_h), (screen_w, screen_h), self.scaling_mode);
        let scaled_w = tex_w * scale;
        let scaled_h = tex_h * scale;
        let clip_x = ((screen_w - scaled_w.min(screen_w)) / 2.0).floor();
        let clip_y = ((screen_h - scaled_h.min(screen_h)) / 2.0).floor();

        let pixel_x = ((physical_position.0 - clip_x) / scale).floor() as isize;
        let pixel_y = ((physical_position.1 - clip_y) / scale).floor() as isize;

        if pixel_x < 0
            || pixel_x >= self.texture_size.0 as isize
            || pixel_y < 0
            || pixel_y >= self.texture_size.1 as isize
        {
            Err((pixel_x, pixel_y))
        } else {
            Ok((pixel_x as usize, pixel_y as usize))
        }
    }

    pub fn clamp_pixel_pos(&self, pos: (isize, isize)) -> (usize, usize) {
        (
            pos.0.clamp(0, self.texture_size.0 as isize - 1) as usize,
            pos.1.clamp(0, self.texture_size.1 as isize - 1) as usize,
        )
    }
}

fn check_texture_size(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> Result<(), FrameRendererError> {
    let limits = device.limits();
    if width == 0 || width > limits.max_texture_dimension_2d {
        return Err(FrameRendererError::InvalidSize { width, height });
    }
    if height == 0 || height > limits.max_texture_dimension_2d {
        return Err(FrameRendererError::InvalidSize { width, height });
    }
    Ok(())
}

fn create_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("evertydesk_frame_texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TEXTURE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    texture_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    uniform_buffer: &wgpu::Buffer,
    label: &str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(texture_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniform_buffer.as_entire_binding(),
            },
        ],
    })
}

fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: 8,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                }],
            }],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

// One full-screen triangle overscanned past [-1, 1]; the fragment shader
// clips to `clip_rect` via the scissor rect. See parasyte/pixels#180.
const VERTEX_DATA: [[f32; 2]; 3] = [[-1.0, -1.0], [3.0, -1.0], [-1.0, 3.0]];

fn vertex_bytes() -> &'static [u8] {
    // SAFETY: `[[f32; 2]; 3]` has no padding and every bit pattern is valid.
    unsafe {
        std::slice::from_raw_parts(
            VERTEX_DATA.as_ptr().cast::<u8>(),
            std::mem::size_of_val(&VERTEX_DATA),
        )
    }
}

/// Scale factor for `texture_size` fit into `screen_size`, mirroring
/// `pixels::renderers::ScalingMatrix::new`.
fn scale_factor(texture_size: (f32, f32), screen_size: (f32, f32), mode: ScalingMode) -> f32 {
    let (texture_width, texture_height) = texture_size;
    let (screen_width, screen_height) = screen_size;
    match mode {
        ScalingMode::PixelPerfect => {
            let width_ratio = (screen_width / texture_width).max(1.0);
            let height_ratio = (screen_height / texture_height).max(1.0);
            width_ratio.min(height_ratio).floor().max(1.0)
        }
        ScalingMode::Fill => {
            let width_ratio = screen_width / texture_width;
            let height_ratio = screen_height / texture_height;
            width_ratio.min(height_ratio)
        }
    }
}

fn clip_rect_for(
    texture_size: (f32, f32),
    screen_size: (f32, f32),
    mode: ScalingMode,
) -> (u32, u32, u32, u32) {
    let (texture_width, texture_height) = texture_size;
    let (screen_width, screen_height) = screen_size;
    let scale = scale_factor(texture_size, screen_size, mode);
    let scaled_width = (texture_width * scale).min(screen_width);
    let scaled_height = (texture_height * scale).min(screen_height);
    let x = ((screen_width - scaled_width) / 2.0) as u32;
    let y = ((screen_height - scaled_height) / 2.0) as u32;
    (x, y, scaled_width as u32, scaled_height as u32)
}

/// Builds the `Locals` uniform buffer: a 4x4 column-major transform matrix
/// (scale + sub-pixel centering translation) followed by
/// `(texture_width, texture_height, 1/texture_width, 1/texture_height)`.
/// Layout must match `scale.wgsl` / `scale_fill.wgsl`'s `Locals` struct.
fn uniform_buffer_bytes(
    texture_size: (f32, f32),
    screen_size: (f32, f32),
    mode: ScalingMode,
) -> Vec<u8> {
    let (texture_width, texture_height) = texture_size;
    let (screen_width, screen_height) = screen_size;
    let scale = scale_factor(texture_size, screen_size, mode);
    let scaled_width = texture_width * scale;
    let scaled_height = texture_height * scale;

    let sw = scaled_width / screen_width;
    let sh = scaled_height / screen_height;
    let tx = (screen_width / 2.0).fract() / screen_width;
    let ty = (screen_height / 2.0).fract() / screen_height;

    #[rustfmt::skip]
    let transform: [f32; 16] = [
        sw,  0.0, 0.0, 0.0,
        0.0, sh,  0.0, 0.0,
        0.0, 0.0, 1.0, 0.0,
        tx,  ty,  0.0, 1.0,
    ];

    let mut bytes = Vec::with_capacity(80);
    for value in transform {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&texture_width.to_le_bytes());
    bytes.extend_from_slice(&texture_height.to_le_bytes());
    bytes.extend_from_slice(&(1.0 / texture_width).to_le_bytes());
    bytes.extend_from_slice(&(1.0 / texture_height).to_le_bytes());
    bytes
}

const SCALE_NEAREST_WGSL: &str = r#"
struct VertexOutput {
    @location(0) tex_coord: vec2<f32>,
    @builtin(position) position: vec4<f32>,
}

struct Locals {
    transform: mat4x4<f32>,
    input_size: vec4<f32>,
}
@group(0) @binding(2) var<uniform> r_locals: Locals;

@vertex
fn vs_main(@location(0) position: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.tex_coord = fma(position, vec2<f32>(0.5, -0.5), vec2<f32>(0.5, 0.5));
    out.position = r_locals.transform * vec4<f32>(position, 0.0, 1.0);
    return out;
}

@group(0) @binding(0) var r_tex_color: texture_2d<f32>;
@group(0) @binding(1) var r_tex_sampler: sampler;

@fragment
fn fs_main(@location(0) tex_coord: vec2<f32>) -> @location(0) vec4<f32> {
    return textureSample(r_tex_color, r_tex_sampler, tex_coord);
}
"#;

const SCALE_FILL_WGSL: &str = r#"
struct VertexOutput {
    @location(0) tex_coord: vec2<f32>,
    @builtin(position) position: vec4<f32>,
}

struct Locals {
    transform: mat4x4<f32>,
    input_size: vec4<f32>,
}
@group(0) @binding(2) var<uniform> r_locals: Locals;

@vertex
fn vs_main(@location(0) position: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.tex_coord = fma(position, vec2<f32>(0.5, -0.5), vec2<f32>(0.5, 0.5)) * r_locals.input_size.xy;
    out.position = r_locals.transform * vec4<f32>(position, 0.0, 1.0);
    return out;
}

@group(0) @binding(0) var r_tex_color: texture_2d<f32>;
@group(0) @binding(1) var r_tex_sampler: sampler;

@fragment
fn fs_main(@location(0) tex_coord: vec2<f32>) -> @location(0) vec4<f32> {
    let half = vec2<f32>(0.5);
    let one = vec2<f32>(1.0);
    let zero = vec2<f32>(0.0);
    let texels_per_pixel = vec2<f32>(dpdx(tex_coord.x), dpdy(tex_coord.y));
    let tex_coord_fract = fract(tex_coord);
    let tex_coord_x = clamp(tex_coord_fract / texels_per_pixel, zero, half)
        + clamp((tex_coord_fract - one) / texels_per_pixel + half, zero, half);
    let tex_coord_final = (floor(tex_coord) + tex_coord_x) * r_locals.input_size.zw;
    return textureSample(r_tex_color, r_tex_sampler, tex_coord_final);
}
"#;
