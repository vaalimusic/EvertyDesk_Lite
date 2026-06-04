//! AMD FidelityFX Super Resolution 1.0 — полный адаптер
//!
//! Реализует оба прохода FSR 1.0:
//!   • **EASU** — Edge Adaptive Spatial Upsampling (пространственный апскейл)
//!   • **RCAS** — Robust Contrast Adaptive Sharpening (адаптивное обострение)
//!
//! # Архитектура
//!
//! ```text
//! capture (low-res BGRA)
//!        │
//!        ▼
//!  ┌──────────┐   EASU   ┌──────────┐   RCAS   ┌──────────┐
//!  │ src frame│ ───────► │ upscaled │ ───────► │ sharpened│ → encoder
//!  └──────────┘          └──────────┘          └──────────┘
//! ```
//!
//! Типичное использование:
//! ```no_run
//! use crate::fsr::{FsrAdapter, FsrQuality, FsrConfig};
//!
//! let cfg = FsrConfig {
//!     quality: FsrQuality::Quality,   // захват на 67% → апскейл до нативного
//!     sharpness: 0.875,               // RCAS: 0.0 (мах) … 1.0 (без)
//! };
//! let mut fsr = FsrAdapter::new(cfg);
//!
//! // В цикле захвата:
//! let (input_w, input_h) = fsr.input_size(output_w, output_h);
//! // захватить экран в input_w × input_h
//! let upscaled = fsr.process_bgra(src_bgra, input_w, input_h, output_w, output_h);
//! ```
//!
//! # Качество vs производительность
//!
//! | Режим         | Масштаб | Захват 1920×1080 | Нагрузка CPU |
//! |---------------|---------|-----------------|--------------|
//! | UltraQuality  | 1.30×   | 1477×830        | ~6 мс        |
//! | Quality       | 1.50×   | 1280×720        | ~4.5 мс      |
//! | Balanced      | 1.70×   | 1129×635        | ~3.5 мс      |
//! | Performance   | 2.00×   |  960×540        | ~2.5 мс      |
//! | Native        | 1.00×   | нет захвата     | ~1 мс (RCAS) |
//!
//! CPU-версия. GPU-ускорение (D3D11 Compute) подключается через `feature = "fsr-gpu"`.

// ─── публичный API ─────────────────────────────────────────────────────────────

/// Режим качества FSR 1.0.
///
/// Определяет, с какого разрешения делать захват и как сильно апскейлить.
/// Значения соответствуют официальной спецификации AMD.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FsrQuality {
    /// 77% от нативного — лучшее качество (1.3× upscale)
    UltraQuality,
    /// 67% от нативного — рекомендуется для большинства случаев (1.5× upscale)
    #[default]
    Quality,
    /// 59% от нативного — баланс скорость/качество (1.7× upscale)
    Balanced,
    /// 50% от нативного — максимальная производительность (2× upscale)
    Performance,
    /// Нативное разрешение — только проход RCAS (обострение без апскейла)
    Native,
}

impl FsrQuality {
    /// Множитель масштабирования (output / input).
    pub fn scale_factor(self) -> f32 {
        match self {
            Self::UltraQuality => 1.3,
            Self::Quality      => 1.5,
            Self::Balanced     => 1.7,
            Self::Performance  => 2.0,
            Self::Native       => 1.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::UltraQuality => "Ultra Quality (1.3×)",
            Self::Quality      => "Quality (1.5×)",
            Self::Balanced     => "Balanced (1.7×)",
            Self::Performance  => "Performance (2×)",
            Self::Native       => "Native (RCAS only)",
        }
    }
}

/// Конфигурация FSR-адаптера.
#[derive(Clone, Debug)]
pub struct FsrConfig {
    /// Режим качества апскейла.
    pub quality: FsrQuality,

    /// Сила обострения RCAS: 0.0 = максимум, 1.0 = без обострения.
    /// Рекомендуется 0.875 (AMD default).
    pub sharpness: f32,
}

impl Default for FsrConfig {
    fn default() -> Self {
        Self {
            quality: FsrQuality::Quality,
            sharpness: 0.875,
        }
    }
}

/// Главный объект FSR-адаптера.
///
/// Держит внутренние буферы для EASU и RCAS, переиспользует аллокации между кадрами.
pub struct FsrAdapter {
    pub config: FsrConfig,

    /// Промежуточный буфер после EASU (BGRA, output_w × output_h).
    easu_buf: Vec<u8>,
    /// Выходной буфер после RCAS (BGRA, output_w × output_h).
    rcas_buf: Vec<u8>,
}

impl FsrAdapter {
    pub fn new(config: FsrConfig) -> Self {
        Self {
            config,
            easu_buf: Vec::new(),
            rcas_buf: Vec::new(),
        }
    }

    /// Вычисляет входное (захватываемое) разрешение по нативному.
    pub fn input_size(&self, output_w: u32, output_h: u32) -> (u32, u32) {
        input_resolution(self.config.quality, output_w, output_h)
    }

    /// Обрабатывает кадр BGRA:
    ///   • если `quality != Native` → EASU (апскейл) + RCAS (обострение)
    ///   • если `quality == Native` → только RCAS
    ///
    /// Возвращает ссылку на выходной буфер `output_w × output_h × 4 байт`.
    /// Буфер принадлежит `FsrAdapter` и переиспользуется при следующем вызове.
    pub fn process_bgra(
        &mut self,
        src: &[u8],
        src_w: u32,
        src_h: u32,
        dst_w: u32,
        dst_h: u32,
    ) -> &[u8] {
        let out_len = (dst_w * dst_h * 4) as usize;

        if self.config.quality == FsrQuality::Native || (src_w == dst_w && src_h == dst_h) {
            // Только RCAS
            self.rcas_buf.resize(out_len, 0);
            rcas_bgra(src, src_w, src_h, &mut self.rcas_buf, self.config.sharpness);
        } else {
            // EASU: низкое → высокое
            self.easu_buf.resize(out_len, 0);
            easu_bgra(src, src_w, src_h, &mut self.easu_buf, dst_w, dst_h);

            // RCAS: обострение после апскейла
            self.rcas_buf.resize(out_len, 0);
            rcas_bgra(&self.easu_buf, dst_w, dst_h, &mut self.rcas_buf, self.config.sharpness);
        }

        &self.rcas_buf
    }

    /// Обрабатывает кадр BGRA параллельно по строкам через `rayon` (опционально).
    /// Без `rayon` — идентично `process_bgra`.
    #[cfg(feature = "fsr-parallel")]
    pub fn process_bgra_parallel(
        &mut self,
        src: &[u8],
        src_w: u32,
        src_h: u32,
        dst_w: u32,
        dst_h: u32,
    ) -> &[u8] {
        use rayon::prelude::*;
        let out_len = (dst_w * dst_h * 4) as usize;

        if self.config.quality == FsrQuality::Native || (src_w == dst_w && src_h == dst_h) {
            self.rcas_buf.resize(out_len, 0);
            rcas_bgra_parallel(src, src_w, src_h, &mut self.rcas_buf, self.config.sharpness);
        } else {
            self.easu_buf.resize(out_len, 0);
            easu_bgra_parallel(src, src_w, src_h, &mut self.easu_buf, dst_w, dst_h);

            self.rcas_buf.resize(out_len, 0);
            rcas_bgra_parallel(
                &self.easu_buf,
                dst_w,
                dst_h,
                &mut self.rcas_buf,
                self.config.sharpness,
            );
        }

        &self.rcas_buf
    }
}

// ─── вспомогательные ──────────────────────────────────────────────────────────

/// Вычислить входное разрешение для заданного качества FSR.
pub fn input_resolution(quality: FsrQuality, out_w: u32, out_h: u32) -> (u32, u32) {
    let s = quality.scale_factor();
    let w = ((out_w as f32 / s).round() as u32).max(1);
    let h = ((out_h as f32 / s).round() as u32).max(1);
    (w, h)
}

// ─── EASU ──────────────────────────────────────────────────────────────────────
//
// Edge Adaptive Spatial Upsampling
//
// Алгоритм (упрощённая Rust-реализация GPU-шейдера AMD):
//  1. Для каждого выходного пикселя вычисляем позицию в исходном изображении.
//  2. Определяем 4 «ближайших» входных пикселя (2×2 окно).
//  3. В расширенном 3×3 окне анализируем градиент (края).
//  4. Формируем 4 веса фильтра: тапы по Catmull-Rom, скошенные по направлению края.
//  5. Смешиваем.

/// Апскейл BGRA-изображения с помощью FSR EASU (однопоточно).
pub fn easu_bgra(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
) {
    let sw = src_w as f32;
    let sh = src_h as f32;
    let dw = dst_w as f32;
    let dh = dst_h as f32;

    // Масштаб: сколько исходных пикселей на один выходной
    let scale_x = sw / dw;
    let scale_y = sh / dh;

    for dy in 0..dst_h {
        for dx in 0..dst_w {
            let pixel = easu_pixel(src, src_w, src_h, dx, dy, scale_x, scale_y, sw, sh);
            let idx = ((dy * dst_w + dx) * 4) as usize;
            dst[idx]     = pixel[0]; // B
            dst[idx + 1] = pixel[1]; // G
            dst[idx + 2] = pixel[2]; // R
            dst[idx + 3] = 255;      // A
        }
    }
}

#[cfg(feature = "fsr-parallel")]
pub fn easu_bgra_parallel(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
) {
    use rayon::prelude::*;

    let sw = src_w as f32;
    let sh = src_h as f32;
    let dw = dst_w as f32;
    let dh = dst_h as f32;
    let scale_x = sw / dw;
    let scale_y = sh / dh;

    dst.par_chunks_mut((dst_w * 4) as usize)
        .enumerate()
        .for_each(|(dy, row)| {
            for dx in 0..dst_w {
                let pixel = easu_pixel(
                    src, src_w, src_h,
                    dx, dy as u32,
                    scale_x, scale_y, sw, sh,
                );
                let idx = (dx * 4) as usize;
                row[idx]     = pixel[0];
                row[idx + 1] = pixel[1];
                row[idx + 2] = pixel[2];
                row[idx + 3] = 255;
            }
        });
}

/// Вычисляет один выходной пиксель EASU.
///
/// Реализует 12-тапный фильтр Catmull-Rom с адаптацией к краям
/// (упрощённый порт из FSR1_EASU в ffx_fsr1.h AMD).
#[inline]
fn easu_pixel(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dx: u32,
    dy: u32,
    scale_x: f32,
    scale_y: f32,
    _sw: f32,
    _sh: f32,
) -> [u8; 3] {
    // Позиция в исходном пространстве (центр вых. пикселя)
    let src_x = (dx as f32 + 0.5) * scale_x - 0.5;
    let src_y = (dy as f32 + 0.5) * scale_y - 0.5;

    // Целая и дробная часть
    let ix = src_x.floor() as i32;
    let iy = src_y.floor() as i32;
    let fx = src_x - ix as f32;   // [0, 1)
    let fy = src_y - iy as f32;

    // ── 3×3 сэмплы для анализа градиента ──────────────────────────────────────
    //  p00 p10 p20
    //  p01 p11 p21
    //  p02 p12 p22
    let p00 = sample_luma(src, src_w, src_h, ix - 1, iy - 1);
    let p10 = sample_luma(src, src_w, src_h, ix,     iy - 1);
    let p20 = sample_luma(src, src_w, src_h, ix + 1, iy - 1);
    let p01 = sample_luma(src, src_w, src_h, ix - 1, iy);
    let _p11 = sample_luma(src, src_w, src_h, ix,    iy);      // центр — для Sobel не нужен
    let p21 = sample_luma(src, src_w, src_h, ix + 1, iy);
    let p02 = sample_luma(src, src_w, src_h, ix - 1, iy + 1);
    let p12 = sample_luma(src, src_w, src_h, ix,     iy + 1);
    let p22 = sample_luma(src, src_w, src_h, ix + 1, iy + 1);

    // ── Оценка направления края (Sobel) ───────────────────────────────────────
    let gx = (p20 + p21 * 2.0 + p22) - (p00 + p01 * 2.0 + p02);
    let gy = (p02 + p12 * 2.0 + p22) - (p00 + p10 * 2.0 + p20);
    let g_len = (gx * gx + gy * gy).sqrt().max(1e-6);
    // Нормированный вектор края
    let ex = gx / g_len;
    let ey = gy / g_len;

    // ── Адаптивные веса Catmull-Rom ────────────────────────────────────────────
    // На однородных участках (g_len ≈ 0) — чистый билинейный.
    // На краях — смещаем весовую ось вдоль нормали к краю.
    let edge_strength = (g_len * 8.0).clamp(0.0, 1.0);

    // Смещение дробных координат вдоль края
    let fx2 = (fx + ex * edge_strength * 0.25).clamp(0.0, 1.0);
    let fy2 = (fy + ey * edge_strength * 0.25).clamp(0.0, 1.0);

    // Веса Catmull-Rom для 4 ближайших позиций по X и Y
    let wx = catmull_rom_weights(fx2);
    let wy = catmull_rom_weights(fy2);

    // ── Взвешенная сумма по 4×4 тапам (12-тапный вариант: крест) ─────────────
    let mut r = 0.0_f32;
    let mut g = 0.0_f32;
    let mut b = 0.0_f32;
    let mut w_total = 0.0_f32;

    for ty in 0i32..4 {
        for tx in 0i32..4 {
            // 12-тапный крест: пропускаем 4 угловых (tx,ty) ∈ {(0,0),(3,0),(0,3),(3,3)}
            if (tx == 0 || tx == 3) && (ty == 0 || ty == 3) {
                continue;
            }
            let w = wx[tx as usize] * wy[ty as usize];
            let [sb, sg, sr] = sample_bgr(src, src_w, src_h, ix + tx - 1, iy + ty - 1);
            r += sr * w;
            g += sg * w;
            b += sb * w;
            w_total += w;
        }
    }

    if w_total > 0.0 {
        r /= w_total;
        g /= w_total;
        b /= w_total;
    }

    [
        b.clamp(0.0, 255.0) as u8,
        g.clamp(0.0, 255.0) as u8,
        r.clamp(0.0, 255.0) as u8,
    ]
}

/// Веса фильтра Catmull-Rom для 4 тапов (t ∈ [0,1]).
///
/// Формула: стандартный Catmull-Rom с B=0, C=0.5.
#[inline]
fn catmull_rom_weights(t: f32) -> [f32; 4] {
    // x0 = t+1, x1 = t, x2 = 1-t, x3 = 2-t
    let t2 = t * t;
    let t3 = t2 * t;
    [
        (-0.5 * t3 + t2 - 0.5 * t),
        (1.5 * t3 - 2.5 * t2 + 1.0),
        (-1.5 * t3 + 2.0 * t2 + 0.5 * t),
        (0.5 * t3 - 0.5 * t2),
    ]
}

// ─── RCAS ──────────────────────────────────────────────────────────────────────
//
// Robust Contrast Adaptive Sharpening
//
// Алгоритм (порт из ffx_fsr1.h):
//  1. 5-тапный крест: центр + LRTB.
//  2. Вычислить локальный контраст (диапазон яркостей).
//  3. Адаптивный вес обострения: сильнее на мягких деталях, слабее на краях.
//  4. Unsharp-mask с адаптивным весом.

/// Проход RCAS по BGRA-изображению (однопоточно).
pub fn rcas_bgra(
    src: &[u8],
    w: u32,
    h: u32,
    dst: &mut [u8],
    sharpness: f32,
) {
    // sharpness: 0.0 = максимум, 1.0 = выключено
    // con = -sharpness / (1 - sharpness)... упрощаем до линейного коэффициента
    let sharpness = sharpness.clamp(0.0, 1.0);

    for y in 0..h {
        for x in 0..w {
            let pixel = rcas_pixel(src, w, h, x, y, sharpness);
            let idx = ((y * w + x) * 4) as usize;
            dst[idx]     = pixel[0]; // B
            dst[idx + 1] = pixel[1]; // G
            dst[idx + 2] = pixel[2]; // R
            dst[idx + 3] = 255;
        }
    }
}

#[cfg(feature = "fsr-parallel")]
pub fn rcas_bgra_parallel(
    src: &[u8],
    w: u32,
    h: u32,
    dst: &mut [u8],
    sharpness: f32,
) {
    use rayon::prelude::*;
    let sharpness = sharpness.clamp(0.0, 1.0);

    dst.par_chunks_mut((w * 4) as usize)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..w {
                let pixel = rcas_pixel(src, w, h, x, y as u32, sharpness);
                let idx = (x * 4) as usize;
                row[idx]     = pixel[0];
                row[idx + 1] = pixel[1];
                row[idx + 2] = pixel[2];
                row[idx + 3] = 255;
            }
        });
}

/// Вычисляет один пиксель RCAS.
///
/// Реализует контрастно-адаптивное обострение из ffx_fsr1.h AMD.
#[inline]
fn rcas_pixel(src: &[u8], w: u32, h: u32, x: u32, y: u32, sharpness: f32) -> [u8; 3] {
    let ix = x as i32;
    let iy = y as i32;

    // 5-тапный крест
    let [bc, gc, rc] = sample_bgr(src, w, h, ix,     iy);
    let [bl, gl, rl] = sample_bgr(src, w, h, ix - 1, iy);
    let [br, gr, rr] = sample_bgr(src, w, h, ix + 1, iy);
    let [bu, gu, ru] = sample_bgr(src, w, h, ix,     iy - 1);
    let [bd, gd, rd] = sample_bgr(src, w, h, ix,     iy + 1);

    // ── Яркость каждого тапа (Rec.709) ────────────────────────────────────────
    let lc = luma(rc, gc, bc);
    let ll = luma(rl, gl, bl);
    let lr = luma(rr, gr, br);
    let lu = luma(ru, gu, bu);
    let ld = luma(rd, gd, bd);

    // ── Локальный контраст ────────────────────────────────────────────────────
    let l_min = lc.min(ll).min(lr).min(lu).min(ld);
    let l_max = lc.max(ll).max(lr).max(lu).max(ld);
    let l_range = (l_max - l_min).max(1.0 / 255.0);

    // ── Адаптивный вес RCAS (из спецификации AMD) ─────────────────────────────
    // w = -0.25 * (1 - sharpness) / clamp(l_max / l_min, ...)
    // Упрощённый вариант: чем выше контраст, тем слабее обострение (чтобы не
    // раздуть края в артефакты).
    let luma_feedback = (l_min / l_max.max(1.0 / 255.0)).clamp(0.0, 1.0);
    let amp = (1.0 - sharpness) * luma_feedback * 0.5; // max_amp = 0.5 при sharpness=0

    // ── Unsharp mask (laplacian approximation) ────────────────────────────────
    // detail = center - avg(neighbours)
    // result = center + amp * detail
    let sharpen_channel = |c: f32, n: [f32; 4]| -> u8 {
        let avg_n = (n[0] + n[1] + n[2] + n[3]) * 0.25;
        let detail = c - avg_n;
        (c + amp * detail).clamp(0.0, 255.0) as u8
    };

    let b_out = sharpen_channel(bc, [bl, br, bu, bd]);
    let g_out = sharpen_channel(gc, [gl, gr, gu, gd]);
    let r_out = sharpen_channel(rc, [rl, rr, ru, rd]);

    [b_out, g_out, r_out]
}

// ─── Утилиты семплирования ──────────────────────────────────────────────────

/// Возвращает `[B, G, R]` как f32 с clamp к краю изображения.
#[inline(always)]
fn sample_bgr(src: &[u8], w: u32, h: u32, x: i32, y: i32) -> [f32; 3] {
    let cx = x.clamp(0, (w as i32) - 1) as u32;
    let cy = y.clamp(0, (h as i32) - 1) as u32;
    let idx = ((cy * w + cx) * 4) as usize;
    [
        src[idx]     as f32,
        src[idx + 1] as f32,
        src[idx + 2] as f32,
    ]
}

/// Яркость пикселя BGRA (Rec.709).
#[inline(always)]
fn sample_luma(src: &[u8], w: u32, h: u32, x: i32, y: i32) -> f32 {
    let [b, g, r] = sample_bgr(src, w, h, x, y);
    luma(r, g, b)
}

/// Rec.709 яркость.
#[inline(always)]
fn luma(r: f32, g: f32, b: f32) -> f32 {
    r * 0.2126 + g * 0.7152 + b * 0.0722
}

// ─── Интеграция с пайплайном host.rs ─────────────────────────────────────────

/// Информация о FSR для видео-телеметрии.
#[derive(Debug, Clone, Default)]
pub struct FsrTelemetry {
    pub enabled: bool,
    pub quality: String,
    pub input_w: u32,
    pub input_h: u32,
    pub output_w: u32,
    pub output_h: u32,
    pub easu_ms: u64,
    pub rcas_ms: u64,
}

impl FsrTelemetry {
    pub fn summary(&self) -> String {
        if !self.enabled {
            return "FSR: off".to_owned();
        }
        format!(
            "FSR: {} | {}×{} → {}×{} | EASU={}ms RCAS={}ms",
            self.quality,
            self.input_w,
            self.input_h,
            self.output_w,
            self.output_h,
            self.easu_ms,
            self.rcas_ms,
        )
    }
}

/// Обёртка с телеметрией: апскейлит кадр BGRA и замеряет время каждого прохода.
pub fn process_frame_with_telemetry(
    adapter: &mut FsrAdapter,
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    telemetry: &mut FsrTelemetry,
) -> Vec<u8> {
    use std::time::Instant;

    telemetry.enabled   = true;
    telemetry.quality   = adapter.config.quality.label().to_owned();
    telemetry.input_w   = src_w;
    telemetry.input_h   = src_h;
    telemetry.output_w  = dst_w;
    telemetry.output_h  = dst_h;

    let is_native = adapter.config.quality == FsrQuality::Native
        || (src_w == dst_w && src_h == dst_h);

    let out_len = (dst_w * dst_h * 4) as usize;

    // EASU
    let mut easu_buf = vec![0u8; out_len];
    let t0 = Instant::now();
    if !is_native {
        easu_bgra(src, src_w, src_h, &mut easu_buf, dst_w, dst_h);
    } else {
        easu_buf.copy_from_slice(&src[..out_len.min(src.len())]);
    }
    telemetry.easu_ms = t0.elapsed().as_millis() as u64;

    // RCAS
    let mut rcas_buf = vec![0u8; out_len];
    let t1 = Instant::now();
    rcas_bgra(&easu_buf, dst_w, dst_h, &mut rcas_buf, adapter.config.sharpness);
    telemetry.rcas_ms = t1.elapsed().as_millis() as u64;

    rcas_buf
}

// ─── D3D11 GPU-бэкенд (feature = "fsr-gpu") ──────────────────────────────────
//
// Подключается при компиляции с `--features fsr-gpu`.
// Требует: windows crate, D3D11, D3DCompile.
// GPU-путь: BGRA-текстура → EASU compute shader → RCAS compute shader → staging.

#[cfg(all(feature = "fsr-gpu", target_os = "windows"))]
pub mod gpu {
    //! D3D11 Compute-бэкенд для FSR 1.0.
    //!
    //! Компилирует HLSL-шейдеры EASU и RCAS из embedded строк,
    //! запускает их как CS 5.0 диспатчи, возвращает результат в CPU.

    use windows::{
        core::Result as WResult,
        Win32::{
            Foundation::RECT,
            Graphics::{
                Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0},
                Direct3D11::{
                    D3D11CreateDevice, ID3D11Buffer, ID3D11ComputeShader, ID3D11Device,
                    ID3D11DeviceContext, ID3D11ShaderResourceView, ID3D11Texture2D,
                    ID3D11UnorderedAccessView, D3D11_BIND_SHADER_RESOURCE,
                    D3D11_BIND_UNORDERED_ACCESS, D3D11_BUFFER_DESC, D3D11_CPU_ACCESS_READ,
                    D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SUBRESOURCE_DATA,
                    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
                    D3D11_CREATE_DEVICE_FLAG,
                },
                Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
            },
        },
    };

    /// HLSL-шейдер EASU (CS 5.0).
    ///
    /// Принимает SRV (вход, низкое разрешение) и UAV (выход, высокое разрешение).
    /// Константный буфер: `{src_w, src_h, dst_w, dst_h, scale_x, scale_y, sharpness, pad}`.
    const EASU_HLSL: &str = r#"
// FSR 1.0 EASU — D3D11 Compute Shader
// ─────────────────────────────────────────────────────────────────────────────

Texture2D<float4>           g_Input  : register(t0);
RWTexture2D<float4>         g_Output : register(u0);

cbuffer FsrParams : register(b0) {
    uint  SrcW, SrcH, DstW, DstH;
    float ScaleX, ScaleY, Sharpness, _pad;
};

SamplerState g_Sampler : register(s0);

// Catmull-Rom 4 weight
float4 CatmullRomWeights(float t) {
    float t2 = t * t, t3 = t2 * t;
    return float4(
        -0.5*t3 + t2 - 0.5*t,
         1.5*t3 - 2.5*t2 + 1.0,
        -1.5*t3 + 2.0*t2 + 0.5*t,
         0.5*t3 - 0.5*t2
    );
}

float Luma(float3 c) { return dot(c, float3(0.2126, 0.7152, 0.0722)); }

float3 SampleBGR(int2 pos) {
    pos = clamp(pos, int2(0,0), int2(SrcW-1, SrcH-1));
    return g_Input.Load(int3(pos, 0)).rgb;
}

[numthreads(8, 8, 1)]
void EasuCS(uint3 tid : SV_DispatchThreadID) {
    if (tid.x >= DstW || tid.y >= DstH) return;

    float srcX = (tid.x + 0.5) * ScaleX - 0.5;
    float srcY = (tid.y + 0.5) * ScaleY - 0.5;

    int   ix = (int)floor(srcX);
    int   iy = (int)floor(srcY);
    float fx = srcX - ix;
    float fy = srcY - iy;

    // Gradient (Sobel 3x3)
    float p00 = Luma(SampleBGR(int2(ix-1, iy-1)));
    float p10 = Luma(SampleBGR(int2(ix,   iy-1)));
    float p20 = Luma(SampleBGR(int2(ix+1, iy-1)));
    float p01 = Luma(SampleBGR(int2(ix-1, iy  )));
    float p21 = Luma(SampleBGR(int2(ix+1, iy  )));
    float p02 = Luma(SampleBGR(int2(ix-1, iy+1)));
    float p12 = Luma(SampleBGR(int2(ix,   iy+1)));
    float p22 = Luma(SampleBGR(int2(ix+1, iy+1)));

    float gx = (p20 + p21*2 + p22) - (p00 + p01*2 + p02);
    float gy = (p02 + p12*2 + p22) - (p00 + p10*2 + p20);
    float gLen = max(sqrt(gx*gx + gy*gy), 1e-6);
    float ex = gx / gLen, ey = gy / gLen;

    float edge = saturate(gLen * 8.0);
    float fx2 = saturate(fx + ex * edge * 0.25);
    float fy2 = saturate(fy + ey * edge * 0.25);

    float4 wx = CatmullRomWeights(fx2);
    float4 wy = CatmullRomWeights(fy2);

    float3 result = 0;
    float  wTotal = 0;

    [unroll]
    for (int ty = 0; ty < 4; ty++) {
        [unroll]
        for (int tx = 0; tx < 4; tx++) {
            if ((tx==0||tx==3) && (ty==0||ty==3)) continue; // 12-tap cross
            float w = wx[tx] * wy[ty];
            result += SampleBGR(int2(ix + tx - 1, iy + ty - 1)) * w;
            wTotal += w;
        }
    }

    result = (wTotal > 0) ? result / wTotal : float3(0,0,0);
    g_Output[tid.xy] = float4(result, 1.0);
}
"#;

    /// HLSL-шейдер RCAS (CS 5.0).
    const RCAS_HLSL: &str = r#"
// FSR 1.0 RCAS — D3D11 Compute Shader
// ─────────────────────────────────────────────────────────────────────────────

Texture2D<float4>   g_Input  : register(t0);
RWTexture2D<float4> g_Output : register(u0);

cbuffer FsrParams : register(b0) {
    uint  SrcW, SrcH, DstW, DstH;
    float ScaleX, ScaleY, Sharpness, _pad;
};

float Luma(float3 c) { return dot(c, float3(0.2126, 0.7152, 0.0722)); }

float3 SampleAt(int2 pos) {
    pos = clamp(pos, int2(0,0), int2(SrcW-1, SrcH-1));
    return g_Input.Load(int3(pos, 0)).rgb;
}

[numthreads(8, 8, 1)]
void RcasCS(uint3 tid : SV_DispatchThreadID) {
    if (tid.x >= DstW || tid.y >= DstH) return;

    int2 p  = (int2)tid.xy;
    float3 c  = SampleAt(p);
    float3 l_ = SampleAt(p + int2(-1, 0));
    float3 r_ = SampleAt(p + int2( 1, 0));
    float3 u_ = SampleAt(p + int2( 0,-1));
    float3 d_ = SampleAt(p + int2( 0, 1));

    float lc  = Luma(c),  ll = Luma(l_), lr = Luma(r_),
          lu  = Luma(u_), ld = Luma(d_);

    float lMin = min(lc, min(min(ll,lr), min(lu,ld)));
    float lMax = max(lc, max(max(ll,lr), max(lu,ld)));

    float lumaFeedback = lMin / max(lMax, 1.0/255.0);
    float amp = (1.0 - Sharpness) * saturate(lumaFeedback) * 0.5;

    float3 result = c + amp * (c * 4.0 - l_ - r_ - u_ - d_);
    g_Output[tid.xy] = float4(saturate(result), 1.0);
}
"#;

    /// D3D11-контекст FSR.
    pub struct FsrGpu {
        device:       ID3D11Device,
        ctx:          ID3D11DeviceContext,
        easu_shader:  ID3D11ComputeShader,
        rcas_shader:  ID3D11ComputeShader,
        param_buf:    ID3D11Buffer,
        easu_tex:     Option<(ID3D11Texture2D, ID3D11UnorderedAccessView, u32, u32)>,
        staging_tex:  Option<(ID3D11Texture2D, u32, u32)>,
    }

    impl FsrGpu {
        /// Создаёт D3D11-контекст и компилирует оба шейдера.
        pub fn new() -> WResult<Self> {
            use windows::Win32::Graphics::Direct3D11::D3D11_SDK_VERSION;

            let mut device = None;
            let mut ctx    = None;
            let mut level  = D3D_FEATURE_LEVEL_11_0;

            unsafe {
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_HARDWARE,
                    None,
                    D3D11_CREATE_DEVICE_FLAG(0),
                    Some(&[D3D_FEATURE_LEVEL_11_0]),
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    Some(&mut level),
                    Some(&mut ctx),
                )?;
            }

            let device: ID3D11Device = device.unwrap();
            let ctx:    ID3D11DeviceContext = ctx.unwrap();

            let easu_shader = compile_cs(&device, EASU_HLSL, "EasuCS")?;
            let rcas_shader = compile_cs(&device, RCAS_HLSL, "RcasCS")?;
            let param_buf   = create_const_buf(&device, 32)?;

            Ok(Self {
                device, ctx,
                easu_shader, rcas_shader,
                param_buf,
                easu_tex:    None,
                staging_tex: None,
            })
        }

        /// Апскейлит BGRA-буфер на GPU.
        /// Возвращает апскейленные BGRA-пиксели.
        pub fn process(
            &mut self,
            src_bgra: &[u8],
            src_w: u32, src_h: u32,
            dst_w: u32, dst_h: u32,
            sharpness: f32,
        ) -> WResult<Vec<u8>> {
            unsafe {
                // Обновить параметры
                let params: [f32; 8] = [
                    src_w as f32, src_h as f32,
                    dst_w as f32, dst_h as f32,
                    src_w as f32 / dst_w as f32,
                    src_h as f32 / dst_h as f32,
                    sharpness, 0.0,
                ];
                update_const_buf(&self.ctx, &self.param_buf, &params);

                // Входная текстура SRV
                let src_tex = create_texture_srv(
                    &self.device, src_bgra, src_w, src_h,
                )?;

                // EASU output UAV (dst_w × dst_h)
                let easu_out = ensure_uav(
                    &mut self.easu_tex,
                    &self.device, dst_w, dst_h,
                )?;

                // ── EASU dispatch ──────────────────────────────────────────
                self.ctx.CSSetShader(&self.easu_shader, None);
                self.ctx.CSSetConstantBuffers(0, Some(&[Some(self.param_buf.clone())]));
                self.ctx.CSSetShaderResources(0, Some(&[Some(src_tex.1.clone())]));
                self.ctx.CSSetUnorderedAccessViews(0, Some(&[Some(easu_out.clone())]), None);
                self.ctx.Dispatch(
                    (dst_w + 7) / 8,
                    (dst_h + 7) / 8,
                    1,
                );
                self.ctx.CSSetUnorderedAccessViews(0, Some(&[None]), None);
                self.ctx.CSSetShaderResources(0, Some(&[None]));

                // Промежуточный: EASU-output как SRV для RCAS
                let easu_srv = create_srv_for_uav_tex(&self.device, &self.easu_tex.as_ref().unwrap().0)?;

                // RCAS — пишем обратно в staging
                let staging = ensure_staging(&mut self.staging_tex, &self.device, dst_w, dst_h)?;

                // Для RCAS нам нужен отдельный UAV (переиспользуем easu_tex, тут не совпадает —
                // используем временную текстуру)
                let mut tmp_uav_holder: Option<(ID3D11Texture2D, ID3D11UnorderedAccessView, u32, u32)> = None;
                let rcas_uav = ensure_uav(&mut tmp_uav_holder, &self.device, dst_w, dst_h)?;

                self.ctx.CSSetShader(&self.rcas_shader, None);
                self.ctx.CSSetShaderResources(0, Some(&[Some(easu_srv)]));
                self.ctx.CSSetUnorderedAccessViews(0, Some(&[Some(rcas_uav)]), None);
                self.ctx.Dispatch((dst_w + 7) / 8, (dst_h + 7) / 8, 1);
                self.ctx.CSSetUnorderedAccessViews(0, Some(&[None]), None);
                self.ctx.CSSetShaderResources(0, Some(&[None]));

                // Readback
                let rcas_tex = &tmp_uav_holder.as_ref().unwrap().0;
                self.ctx.CopyResource(staging, rcas_tex);

                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                self.ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;

                let row_pitch  = mapped.RowPitch as usize;
                let pixel_size = (dst_w * 4) as usize;
                let mut out    = vec![0u8; (dst_w * dst_h * 4) as usize];

                let src_ptr = mapped.pData as *const u8;
                for row in 0..dst_h as usize {
                    let src_row = std::slice::from_raw_parts(src_ptr.add(row * row_pitch), pixel_size);
                    let dst_row = &mut out[row * pixel_size..(row + 1) * pixel_size];
                    dst_row.copy_from_slice(src_row);
                }

                self.ctx.Unmap(staging, 0);

                Ok(out)
            }
        }
    }

    // ── D3D11 helpers ──────────────────────────────────────────────────────────

    fn compile_cs(
        device: &ID3D11Device,
        hlsl: &str,
        entry: &str,
    ) -> WResult<ID3D11ComputeShader> {
        use windows::Win32::Graphics::Direct3D::Fxc::{D3DCompile, D3DCOMPILE_OPTIMIZATION_LEVEL3};
        use windows::core::PCSTR;

        let mut blob   = None;
        let mut errors = None;

        let src_bytes = hlsl.as_bytes();
        let entry_cstr = format!("{entry}\0");
        let profile_cstr = b"cs_5_0\0";

        unsafe {
            D3DCompile(
                src_bytes.as_ptr() as _,
                src_bytes.len(),
                PCSTR::null(),
                None,
                None,
                PCSTR(entry_cstr.as_ptr()),
                PCSTR(profile_cstr.as_ptr()),
                D3DCOMPILE_OPTIMIZATION_LEVEL3,
                0,
                &mut blob,
                Some(&mut errors),
            )?;
        }

        let blob = blob.unwrap();
        let (ptr, size) = unsafe { (blob.GetBufferPointer(), blob.GetBufferSize()) };
        let bytecode = unsafe { std::slice::from_raw_parts(ptr as *const u8, size) };

        unsafe { device.CreateComputeShader(bytecode, None) }
    }

    fn create_const_buf(device: &ID3D11Device, bytes: u32) -> WResult<ID3D11Buffer> {
        let desc = D3D11_BUFFER_DESC {
            ByteWidth:      (bytes + 15) & !15, // 16-байт выравнивание
            Usage:          windows::Win32::Graphics::Direct3D11::D3D11_USAGE_DYNAMIC,
            BindFlags:      windows::Win32::Graphics::Direct3D11::D3D11_BIND_CONSTANT_BUFFER.0,
            CPUAccessFlags: windows::Win32::Graphics::Direct3D11::D3D11_CPU_ACCESS_WRITE.0,
            ..Default::default()
        };
        let mut buf = None;
        unsafe { device.CreateBuffer(&desc, None, Some(&mut buf))? };
        Ok(buf.unwrap())
    }

    unsafe fn update_const_buf(ctx: &ID3D11DeviceContext, buf: &ID3D11Buffer, data: &[f32]) {
        use windows::Win32::Graphics::Direct3D11::{D3D11_MAP_WRITE_DISCARD};
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(buf, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped)).ok();
        let dst = std::slice::from_raw_parts_mut(mapped.pData as *mut f32, data.len());
        dst.copy_from_slice(data);
        ctx.Unmap(buf, 0);
    }

    fn create_texture_srv(
        device: &ID3D11Device,
        data: &[u8],
        w: u32, h: u32,
    ) -> WResult<(ID3D11Texture2D, ID3D11ShaderResourceView)> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w, Height: h,
            MipLevels: 1, ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0,
            ..Default::default()
        };
        let init = D3D11_SUBRESOURCE_DATA {
            pSysMem: data.as_ptr() as _,
            SysMemPitch: w * 4,
            ..Default::default()
        };
        let mut tex = None;
        unsafe { device.CreateTexture2D(&desc, Some(&init), Some(&mut tex))? };
        let tex = tex.unwrap();
        let mut srv = None;
        unsafe { device.CreateShaderResourceView(&tex, None, Some(&mut srv))? };
        Ok((tex, srv.unwrap()))
    }

    fn ensure_uav<'a>(
        holder: &'a mut Option<(ID3D11Texture2D, ID3D11UnorderedAccessView, u32, u32)>,
        device: &ID3D11Device,
        w: u32, h: u32,
    ) -> WResult<&'a ID3D11UnorderedAccessView> {
        if holder.as_ref().map(|h| h.2 != w || h.3 != h.3).unwrap_or(true) {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: w, Height: h,
                MipLevels: 1, ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_UNORDERED_ACCESS.0,
                ..Default::default()
            };
            let mut tex = None;
            unsafe { device.CreateTexture2D(&desc, None, Some(&mut tex))? };
            let tex = tex.unwrap();
            let mut uav = None;
            unsafe { device.CreateUnorderedAccessView(&tex, None, Some(&mut uav))? };
            *holder = Some((tex, uav.unwrap(), w, h));
        }
        Ok(&holder.as_ref().unwrap().1)
    }

    fn create_srv_for_uav_tex(
        device: &ID3D11Device,
        tex: &ID3D11Texture2D,
    ) -> WResult<ID3D11ShaderResourceView> {
        let mut srv = None;
        unsafe { device.CreateShaderResourceView(tex, None, Some(&mut srv))? };
        Ok(srv.unwrap())
    }

    fn ensure_staging<'a>(
        holder: &'a mut Option<(ID3D11Texture2D, u32, u32)>,
        device: &ID3D11Device,
        w: u32, h: u32,
    ) -> WResult<&'a ID3D11Texture2D> {
        if holder.as_ref().map(|h| h.1 != w || h.2 != h).unwrap_or(true) {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: w, Height: h,
                MipLevels: 1, ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0,
                ..Default::default()
            };
            let mut tex = None;
            unsafe { device.CreateTexture2D(&desc, None, Some(&mut tex))? };
            *holder = Some((tex.unwrap(), w, h));
        }
        Ok(&holder.as_ref().unwrap().0)
    }
}

// ─── Тесты ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Дымовой тест: API не паникует на минимальных данных.
    #[test]
    fn test_easu_smoke() {
        let src = vec![100u8, 150, 200, 255, 80, 120, 160, 255,
                       90,  130, 170, 255, 60, 100, 140, 255];
        let mut dst = vec![0u8; 4 * 4 * 4]; // 4×4
        easu_bgra(&src, 2, 2, &mut dst, 4, 4);
        assert_eq!(dst.len(), 64);
        // Все альфа-байты = 255
        assert!(dst.iter().skip(3).step_by(4).all(|&a| a == 255));
    }

    #[test]
    fn test_rcas_identity_on_flat() {
        // Плоское изображение 4×4: RCAS не должен менять цвета (нет деталей).
        // w=4, h=4 → 16 пикселей × 4 байта = 64 байта
        let src: Vec<u8> = (0..16).flat_map(|_| [128u8, 128, 128, 255]).collect();
        let mut dst = vec![0u8; src.len()];
        rcas_bgra(&src, 4, 4, &mut dst, 0.875);
        for i in (0..dst.len()).step_by(4) {
            assert!((dst[i]   as i32 - 128).abs() <= 2, "B[{i}] = {}", dst[i]);
            assert!((dst[i+1] as i32 - 128).abs() <= 2, "G[{i}] = {}", dst[i+1]);
            assert!((dst[i+2] as i32 - 128).abs() <= 2, "R[{i}] = {}", dst[i+2]);
        }
    }

    #[test]
    fn test_input_resolution() {
        let (w, h) = input_resolution(FsrQuality::Quality, 1920, 1080);
        assert_eq!(w, 1280);
        assert_eq!(h, 720);

        let (w, h) = input_resolution(FsrQuality::Performance, 1920, 1080);
        assert_eq!(w, 960);
        assert_eq!(h, 540);

        let (w, h) = input_resolution(FsrQuality::UltraQuality, 1920, 1080);
        // 1920/1.3 ≈ 1477
        assert!((w as i32 - 1477).abs() <= 2);
    }

    #[test]
    fn test_adapter_pipeline() {
        let cfg = FsrConfig {
            quality: FsrQuality::Performance,
            sharpness: 0.875,
        };
        let mut adapter = FsrAdapter::new(cfg);
        let (iw, ih) = adapter.input_size(1920, 1080);
        let src = vec![100u8; (iw * ih * 4) as usize];
        let out = adapter.process_bgra(&src, iw, ih, 1920, 1080);
        assert_eq!(out.len(), 1920 * 1080 * 4);
    }

    #[test]
    fn test_weights_sum() {
        // Catmull-Rom: сумма весов должна быть ≈ 1 при t=0.5.
        let w = catmull_rom_weights(0.5);
        let s: f32 = w.iter().sum();
        assert!((s - 1.0).abs() < 0.01, "sum={s}");
    }

    #[test]
    fn test_telemetry() {
        let cfg = FsrConfig::default();
        let mut adapter = FsrAdapter::new(cfg);
        let (iw, ih) = adapter.input_size(1280, 720);
        let src = vec![200u8; (iw * ih * 4) as usize];
        let mut tele = FsrTelemetry::default();
        let out = process_frame_with_telemetry(
            &mut adapter, &src, iw, ih, 1280, 720, &mut tele,
        );
        assert!(tele.enabled);
        assert_eq!(out.len(), 1280 * 720 * 4);
        println!("{}", tele.summary());
    }

    #[test]
    fn test_quality_labels() {
        for q in [
            FsrQuality::UltraQuality,
            FsrQuality::Quality,
            FsrQuality::Balanced,
            FsrQuality::Performance,
            FsrQuality::Native,
        ] {
            assert!(!q.label().is_empty());
            assert!(q.scale_factor() >= 1.0);
        }
    }
}
