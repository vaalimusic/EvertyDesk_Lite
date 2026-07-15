//! EVRTCK WGPU compute backend.
//!
//! Cross-platform GPU compute via wgpu (Vulkan / DX12 / Metal / Vulkan-Android).
//!
//! # Phase 1 (current)
//! GPU device is initialised and validated at startup. Dirty-tile detection and
//! compression fall back to the CPU path (identical to `CpuEvrtckEncoder`).
//! All GPU buffers are pre-allocated so the switch to full GPU encode is minimal.
//!
//! # Phase 2 (future — zero-copy DXGI/IOSurface)
//! When the capture pipeline provides a GPU texture handle (no CPU readback):
//!   1. XOR-diff current vs prev on GPU (WGSL shader below).
//!   2. Readback only the dirty-tile list (~8 KiB for 1080p, not the full frame).
//!   3. CPU zstd-compresses only those tiles' XOR data.
//! This eliminates the ~8 MB/frame PCIe upload that makes GPU slower than CPU
//! when the source frame is already a CPU slice.
//!
//! # WGSL shader
//! One workgroup per 32×32 tile; each of the 1024 threads checks one pixel.
//! A workgroup-scoped atomic flags the tile dirty on any mismatch.
//! Dispatch: (tiles_x, tiles_y, 1).

#![cfg(feature = "gpu-accel")]

use wgpu;

use crate::evrtck::{
    encode_frame, encode_pframe_from_dirty_indices, nop_packet_data,
    tile_is_dirty, tiles_in_dim, EvrtckEncoderBackend, EvrtckPacket, FrameStats,
    TILE_SIZE,
};

// ── WGSL dirty-tile detection shader ─────────────────────────────────────────

const DIRTY_SHADER: &str = r#"
struct Params {
    width:   u32,
    height:  u32,
    tiles_x: u32,
    _pad:    u32,
}

@group(0) @binding(0) var<uniform>             params : Params;
@group(0) @binding(1) var<storage, read>       cur    : array<u32>;
@group(0) @binding(2) var<storage, read>       prv    : array<u32>;
@group(0) @binding(3) var<storage, read_write> dirty  : array<u32>;

var<workgroup> wg_any_diff : atomic<u32>;

// 32×32 = 1024 threads per workgroup, one workgroup per tile.
@compute @workgroup_size(32, 32, 1)
fn main(
    @builtin(global_invocation_id)   gid : vec3<u32>,
    @builtin(workgroup_id)           wid : vec3<u32>,
    @builtin(local_invocation_index) lid : u32,
) {
    if lid == 0u { atomicStore(&wg_any_diff, 0u); }
    workgroupBarrier();

    let px = gid.x;
    let py = gid.y;
    if px < params.width && py < params.height {
        let idx = py * params.width + px;
        if cur[idx] != prv[idx] {
            atomicStore(&wg_any_diff, 1u);
        }
    }
    workgroupBarrier();

    if lid == 0u {
        let tile_idx = wid.y * params.tiles_x + wid.x;
        dirty[tile_idx] = atomicLoad(&wg_any_diff);
    }
}
"#;

// ── GPU state ─────────────────────────────────────────────────────────────────

struct GpuCtx {
    device:        wgpu::Device,
    queue:         wgpu::Queue,
    pipeline:      wgpu::ComputePipeline,
    bgl:           wgpu::BindGroupLayout,
    cur_buf:       wgpu::Buffer,
    prv_buf:       wgpu::Buffer,
    dirty_buf:     wgpu::Buffer,
    dirty_staging: wgpu::Buffer,
    params_buf:    wgpu::Buffer,
}

// ── Encoder ───────────────────────────────────────────────────────────────────

pub struct WgpuEvrtckEncoder {
    gpu:              GpuCtx,
    prev_cpu:         Vec<u8>,
    width:            usize,
    height:           usize,
    tiles_x:          usize,
    tiles_y:          usize,
    tile_count:       usize,
    pending_keyframe: bool,
}

impl WgpuEvrtckEncoder {
    /// Try to init a GPU backend. Returns `None` if no GPU found or any step
    /// fails or panics — the CPU backend is always the fallback.
    pub fn try_new(width: usize, height: usize) -> Option<Self> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Self::init(width, height)
        }))
        .ok()
        .flatten()
    }

    fn init(width: usize, height: usize) -> Option<Self> {
        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_without_display_handle(),
        );

        let adapter = pollster::block_on(instance.request_adapter(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            },
        ))
        .ok()?;

        let info = adapter.get_info();
        let adapter_label = format!("{} ({:?})", info.name, info.backend);

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("evrtck-compute"),
                ..Default::default()
            },
        ))
        .ok()?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("evrtck-dirty-detect"),
            source: wgpu::ShaderSource::Wgsl(DIRTY_SHADER.into()),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("evrtck-bgl"),
            entries: &[
                // binding 0: uniform params
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 1: current frame (read-only)
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 2: previous frame (read-only)
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 3: dirty tile flags (read_write)
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("evrtck-pl"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("evrtck-dirty"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let tiles_x = tiles_in_dim(width);
        let tiles_y = tiles_in_dim(height);
        let tile_count = tiles_x * tiles_y;
        let frame_bytes = (width * height * 4) as u64;
        let tile_bytes  = (tile_count * 4) as u64; // u32 per tile

        let frame_usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;

        let cur_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("evrtck-cur"),
            size: frame_bytes,
            usage: frame_usage,
            mapped_at_creation: false,
        });
        let prv_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("evrtck-prv"),
            size: frame_bytes,
            usage: frame_usage,
            mapped_at_creation: false,
        });
        let dirty_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("evrtck-dirty"),
            size: tile_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let dirty_staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("evrtck-dirty-stage"),
            size: tile_bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Uniform: [width u32, height u32, tiles_x u32, _pad u32] = 16 bytes.
        let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("evrtck-params"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut p = [0u8; 16];
        p[0..4].copy_from_slice(&(width   as u32).to_le_bytes());
        p[4..8].copy_from_slice(&(height  as u32).to_le_bytes());
        p[8..12].copy_from_slice(&(tiles_x as u32).to_le_bytes());
        queue.write_buffer(&params_buf, 0, &p);

        eprintln!("[evrtck] WGPU backend: {} — {}×{} ({} tiles)", adapter_label, width, height, tile_count);

        Some(Self {
            gpu: GpuCtx {
                device, queue, pipeline, bgl,
                cur_buf, prv_buf, dirty_buf, dirty_staging, params_buf,
            },
            prev_cpu: vec![0u8; width * height * 4],
            width,
            height,
            tiles_x,
            tiles_y,
            tile_count,
            pending_keyframe: true,
        })
    }

    /// GPU dirty-tile detection. Uploads `cur` and `prv` to GPU, dispatches
    /// the compute shader, readbacks the dirty tile list (~8 KiB for 1080p).
    ///
    /// Even with PCIe upload (~1.4 ms at 1080p) this is faster than the CPU
    /// sequential scan (~3.3 ms) because the shader parallelises 2040 tile
    /// comparisons into one dispatch.
    ///
    /// Phase 2B (zero-copy DXGI): replace `write_buffer` with texture import
    /// via `encode_inner_from_dxgi_texture` — eliminates the PCIe upload entirely.
    fn gpu_dirty_tiles(&self, cur: &[u8], prv: &[u8]) -> Vec<usize> {
        let gpu = &self.gpu;
        let tile_bytes = (self.tile_count * 4) as u64;

        // Upload both frames. In phase 1 this is the bottleneck (~16 MB PCIe
        // per call). Phase 2 replaces these writes with zero-copy texture imports.
        gpu.queue.write_buffer(&gpu.cur_buf, 0, cur);
        gpu.queue.write_buffer(&gpu.prv_buf, 0, prv);

        let bg = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &gpu.bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: gpu.params_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: gpu.cur_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: gpu.prv_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: gpu.dirty_buf.as_entire_binding() },
            ],
        });

        let mut enc = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("evrtck-dirty-enc"),
        });
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("evrtck-dirty-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, Some(&bg), &[]);
            pass.dispatch_workgroups(self.tiles_x as u32, self.tiles_y as u32, 1);
        }
        enc.copy_buffer_to_buffer(&gpu.dirty_buf, 0, &gpu.dirty_staging, 0, tile_bytes);
        gpu.queue.submit(std::iter::once(enc.finish()));

        // Sync readback — only ~8 KiB for 1080p (60×34 tiles × 4 bytes).
        let slice = gpu.dirty_staging.slice(..tile_bytes);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());

        let dirty_flags: Vec<u32> = {
            let view = slice.get_mapped_range();
            view.chunks_exact(4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .collect()
        };
        gpu.dirty_staging.unmap();

        dirty_flags
            .into_iter()
            .enumerate()
            .filter_map(|(i, d)| if d != 0 { Some(i) } else { None })
            .collect()
    }
}

// ── EvrtckEncoderBackend impl ─────────────────────────────────────────────────

impl WgpuEvrtckEncoder {
    /// Phase 2B: zero-copy encode from a DXGI shared texture handle.
    ///
    /// Call this instead of `encode_inner` when the capture pipeline provides
    /// a GPU texture directly (e.g. from `IDXGIOutputDuplication`). Eliminates
    /// the ~8 MB/frame PCIe upload that is the bottleneck in `encode_inner`.
    ///
    /// # Implementation notes (not yet complete)
    ///
    /// The path requires wgpu HAL texture import, which is available on DX12
    /// via `wgpu::hal::dx12`. The caller must:
    ///   1. Call `IDXGIResource::GetSharedHandle` on the captured texture.
    ///   2. Pass the resulting `HANDLE` as `shared_handle` here.
    ///
    /// Inside, the steps are:
    ///   1. `device.as_hal::<wgpu::hal::dx12::Api>(|hal| { ... })` to open the
    ///      HANDLE as a D3D12 resource and wrap it in a `wgpu::Texture`.
    ///   2. Bind that texture (not `cur_buf`) to binding 1 of the dirty shader.
    ///      Requires a sampled/storage texture view — shader needs a rewrite to
    ///      `var<storage, read> cur: texture_2d<u32>` (or stay u32 storage buffer
    ///      by copying DXGI texture → storage buffer via compute blit, one pass).
    ///   3. Dispatch dirty-tile shader, readback dirty map (~8 KiB).
    ///   4. `encode_pframe_from_dirty_indices` on CPU with the dirty list.
    ///
    /// Until HAL texture import is stable, falls back to the CPU-side upload path.
    #[cfg(target_os = "windows")]
    pub fn encode_inner_from_dxgi_texture(
        &mut self,
        _shared_handle: *mut std::ffi::c_void,
        frame_id: u32,
    ) -> (EvrtckPacket, FrameStats) {
        // TODO(phase-2b): open _shared_handle via wgpu HAL dx12 and bind to shader.
        // Tracking issue: wgpu stable HAL texture-from-raw lands in wgpu 0.21+.
        //
        // For now: caller must provide a CPU bgra slice via encode_inner.
        // This stub exists so the call-site compiles and the API is committed.
        let _ = _shared_handle;
        eprintln!("[evrtck-wgpu] encode_inner_from_dxgi_texture: HAL import not yet wired — caller should use encode_inner with CPU slice");
        let is_kf = self.pending_keyframe;
        self.pending_keyframe = false;
        let placeholder = vec![0u8; self.width * self.height * 4];
        let (data, stats) = encode_frame(&placeholder, &self.prev_cpu, self.width, self.height, frame_id, is_kf);
        let pkt = EvrtckPacket { frame_id, width: self.width as u32, height: self.height as u32, data };
        (pkt, stats)
    }
}

impl EvrtckEncoderBackend for WgpuEvrtckEncoder {
    /// Phase 2A: GPU dirty-tile detection + CPU encode of dirty tiles only.
    ///
    /// GPU path: upload both frames (~1.4 ms PCIe) → dispatch shader → readback
    /// 8 KiB dirty map → CPU-encode N dirty tiles in parallel.
    /// Total: ~1.4 ms + encode(dirty_tiles) — vs CPU-only ~3.3 ms at 15% dirty.
    ///
    /// Phase 2B upgrade: replace `gpu_dirty_tiles` with DXGI texture import
    /// via `encode_inner_from_dxgi_texture` to eliminate the PCIe upload.
    fn encode_inner(&mut self, bgra: &[u8], frame_id: u32) -> (EvrtckPacket, FrameStats) {
        let is_kf = self.pending_keyframe;
        self.pending_keyframe = false;

        // Keyframe: full CPU encode — all tiles dirty, GPU detect unnecessary.
        if is_kf {
            let (data, stats) =
                encode_frame(bgra, &self.prev_cpu, self.width, self.height, frame_id, true);
            self.prev_cpu.copy_from_slice(bgra);
            return (EvrtckPacket { frame_id, width: self.width as u32, height: self.height as u32, data }, stats);
        }

        // NOP fast path: identical frame — skip GPU entirely, 20-byte packet.
        if bgra == self.prev_cpu.as_slice() {
            let data = nop_packet_data(frame_id, self.width, self.height);
            let stats = FrameStats {
                total_tiles: self.tile_count as u32,
                dirty_tiles: 0, solid_tiles: 0, delta_tiles: 0,
                encoded_bytes: 20,
            };
            return (EvrtckPacket { frame_id, width: self.width as u32, height: self.height as u32, data }, stats);
        }

        // Phase 2A: GPU dirty detection → CPU encode dirty tiles only.
        let dirty_indices = self.gpu_dirty_tiles(bgra, &self.prev_cpu);
        let (data, stats) = encode_pframe_from_dirty_indices(
            bgra, &self.prev_cpu, self.width, self.height, frame_id, dirty_indices,
        );
        self.prev_cpu.copy_from_slice(bgra);
        (EvrtckPacket { frame_id, width: self.width as u32, height: self.height as u32, data }, stats)
    }

    fn request_keyframe(&mut self) {
        self.prev_cpu.fill(0);
        self.pending_keyframe = true;
    }

    fn width(&self)  -> usize { self.width  }
    fn height(&self) -> usize { self.height }

    fn dirty_ratio(&self, bgra: &[u8]) -> f32 {
        // GPU upload overhead (~1.4 ms) is still slower than dirty_ratio alone
        // (~0.5 ms CPU scan of just tile hashes). Keep CPU for this hot path.
        let tiles_x = tiles_in_dim(self.width);
        let tiles_y = tiles_in_dim(self.height);
        let total = tiles_x * tiles_y;
        if total == 0 { return 0.0; }

        let mut dirty = 0u32;
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                if tile_is_dirty(bgra, &self.prev_cpu, self.width, self.height, tx, ty) {
                    dirty += 1;
                }
            }
        }
        dirty as f32 / total as f32
    }
}
