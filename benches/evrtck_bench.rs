//! EVRTCK codec benchmarks.
//!
//! Run: cargo bench --bench evrtck_bench
//!
//! Scenarios
//! ─────────
//! keyframe_720p / _1080p / _4k
//!     Full first-frame encode: all tiles dirty, prev=black.
//!     Bottleneck: zstd compression of raw pixel data.
//!
//! pframe_static   (0% dirty)
//! pframe_sparse   (5% dirty  — cursor blink, status bar update)
//! pframe_typing   (15% dirty — terminal, code editor)
//! pframe_scroll   (50% dirty — browser scroll, code review)
//! pframe_video    (90% dirty — embedded video, animation)
//!     P-frame encode at 1080p. Dirty tiles use zstd/ZRLE on XOR delta.
//!     Static tiles cost ~1 bit each in the tile map — pure overhead.
//!
//! dirty_ratio_scan_1080p
//!     Pre-scan before encode (no compression). Called by video_pipeline
//!     to decide whether EVRTCK is worthwhile for this frame.
//!
//! roundtrip_1080p_typing
//!     Full encode + decode cycle at 15% dirty. Decode is O(dirty_tiles).

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use evertydesk_core::evrtck::{EvrtckDecoder, EvrtckEncoder};
use std::hint::black_box;

// ── Frame generators ─────────────────────────────────────────────────────────

/// Solid-color BGRA frame (simulates a completely static screen).
fn solid_frame(w: usize, h: usize, color: [u8; 4]) -> Vec<u8> {
    color.iter().cycle().take(w * h * 4).copied().collect()
}

/// Gradient BGRA frame — varied content for keyframe compression.
fn gradient_frame(w: usize, h: usize) -> Vec<u8> {
    (0..h)
        .flat_map(|y| {
            (0..w).flat_map(move |x| {
                let r = ((x * 255) / w) as u8;
                let g = ((y * 255) / h) as u8;
                let b = (((x + y) * 255) / (w + h)) as u8;
                [b, g, r, 255u8] // BGRA
            })
        })
        .collect()
}

/// A frame where `dirty_fraction` of 32×32 tiles differ from `base`.
/// Dirty tiles are filled with a solid contrasting color.
fn dirty_frame(base: &[u8], w: usize, h: usize, dirty_fraction: f32) -> Vec<u8> {
    use evertydesk_core::evrtck::TILE_SIZE;
    let mut frame = base.to_vec();
    let tiles_x = (w + TILE_SIZE - 1) / TILE_SIZE;
    let tiles_y = (h + TILE_SIZE - 1) / TILE_SIZE;
    let total_tiles = tiles_x * tiles_y;
    let dirty_count = (total_tiles as f32 * dirty_fraction) as usize;

    for tile_idx in 0..dirty_count {
        let tx = tile_idx % tiles_x;
        let ty = tile_idx / tiles_x;
        let x0 = tx * TILE_SIZE;
        let y0 = ty * TILE_SIZE;
        let x1 = (x0 + TILE_SIZE).min(w);
        let y1 = (y0 + TILE_SIZE).min(h);
        for y in y0..y1 {
            for x in x0..x1 {
                let off = (y * w + x) * 4;
                frame[off]     = 255 - base[off];     // invert B
                frame[off + 1] = 255 - base[off + 1]; // invert G
                frame[off + 2] = 255 - base[off + 2]; // invert R
                // A unchanged
            }
        }
    }
    frame
}

// ── Benchmark groups ─────────────────────────────────────────────────────────

fn bench_keyframes(c: &mut Criterion) {
    let resolutions = [
        ("720p",  1280usize,  720usize),
        ("1080p", 1920,       1080),
        ("4k",    3840,       2160),
    ];

    let mut group = c.benchmark_group("keyframe");
    for (name, w, h) in &resolutions {
        let frame = gradient_frame(*w, *h);
        let raw_bytes = (w * h * 4) as u64;
        group.throughput(Throughput::Bytes(raw_bytes));
        group.bench_with_input(BenchmarkId::from_parameter(name), &(*w, *h, &frame), |b, (w, h, frame)| {
            b.iter(|| {
                let mut enc = EvrtckEncoder::new(*w, *h);
                black_box(enc.encode(black_box(frame), 1))
            });
        });
    }
    group.finish();
}

fn bench_pframes(c: &mut Criterion) {
    let (w, h) = (1920usize, 1080usize);
    let base = solid_frame(w, h, [30, 30, 30, 255]);
    let raw_bytes = (w * h * 4) as u64;

    let scenarios: &[(&str, f32)] = &[
        ("static_0pct",  0.00),
        ("sparse_5pct",  0.05),
        ("typing_15pct", 0.15),
        ("scroll_50pct", 0.50),
        ("video_90pct",  0.90),
    ];

    let mut group = c.benchmark_group("pframe_1080p");
    group.throughput(Throughput::Bytes(raw_bytes));

    for (name, dirty_frac) in scenarios {
        let delta = dirty_frame(&base, w, h, *dirty_frac);

        group.bench_with_input(BenchmarkId::from_parameter(name), &delta, |b, delta| {
            b.iter_batched(
                || {
                    // Each iteration needs a fresh encoder with prev=base already set.
                    let mut enc = EvrtckEncoder::new(w, h);
                    enc.encode(&base, 1); // establish prev
                    enc
                },
                |mut enc| black_box(enc.encode(black_box(delta), 2)),
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn bench_nop_static_frame(c: &mut Criterion) {
    // Measures the NOP fast path: identical cur==prev, no rayon scan.
    // Expected: ~0.15 ms (one memcmp) vs 3.17 ms (old tile scan).
    let (w, h) = (1920usize, 1080usize);
    let frame = solid_frame(w, h, [30, 30, 30, 255]);

    let mut group = c.benchmark_group("nop_static");
    group.throughput(Throughput::Bytes((w * h * 4) as u64));

    group.bench_function("1080p_identical", |b| {
        b.iter_batched(
            || {
                let mut enc = EvrtckEncoder::new(w, h);
                enc.encode(&frame, 1); // establish prev
                enc
            },
            |mut enc| black_box(enc.encode(black_box(&frame), 2)),
            criterion::BatchSize::SmallInput,
        );
    });
    group.finish();
}

fn bench_dirty_ratio_scan(c: &mut Criterion) {
    let (w, h) = (1920usize, 1080usize);
    let base  = solid_frame(w, h, [30, 30, 30, 255]);
    let frame = dirty_frame(&base, w, h, 0.15);

    let mut group = c.benchmark_group("dirty_ratio_scan");
    group.throughput(Throughput::Bytes((w * h * 4) as u64));

    group.bench_function("1080p_15pct", |b| {
        let enc = {
            let mut e = EvrtckEncoder::new(w, h);
            e.encode(&base, 1); // establish prev
            e
        };
        b.iter(|| black_box(enc.dirty_ratio(black_box(&frame))));
    });
    group.finish();
}

fn bench_roundtrip(c: &mut Criterion) {
    let (w, h) = (1920usize, 1080usize);
    let base  = solid_frame(w, h, [30, 30, 30, 255]);
    let frame = dirty_frame(&base, w, h, 0.15);

    let mut group = c.benchmark_group("roundtrip");
    group.throughput(Throughput::Bytes((w * h * 4) as u64));

    group.bench_function("1080p_typing_15pct", |b| {
        b.iter_batched(
            || {
                let mut enc = EvrtckEncoder::new(w, h);
                let kf = enc.encode(&base, 1);
                let mut dec = EvrtckDecoder::new();
                dec.decode_wire(&kf.data).unwrap();
                (enc, dec)
            },
            |(mut enc, mut dec)| {
                let pkt = enc.encode(black_box(&frame), 2);
                // decode_wire returns &[u8] into dec — return length to prevent
                // the compiler from eliding the work while keeping dec owned.
                let pixels = dec.decode_wire(&pkt.data).unwrap();
                black_box(pixels.len())
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.finish();
}

fn bench_pframe_sizes(c: &mut Criterion) {
    // Compression ratio benchmark: measure output size across dirty fractions.
    // Reported as throughput (encoded bytes out / raw bytes in).
    let (w, h) = (1920usize, 1080usize);
    let base = solid_frame(w, h, [30, 30, 30, 255]);

    let scenarios: &[(&str, f32)] = &[
        ("0pct",  0.00),
        ("5pct",  0.05),
        ("15pct", 0.15),
        ("50pct", 0.50),
        ("90pct", 0.90),
    ];

    let mut group = c.benchmark_group("encode_size");
    for (name, frac) in scenarios {
        let frame = dirty_frame(&base, w, h, *frac);
        let mut enc = EvrtckEncoder::new(w, h);
        enc.encode(&base, 1);
        let pkt = enc.encode(&frame, 2);
        // Throughput = encoded bytes (what we measure the cost for)
        group.throughput(Throughput::Bytes(pkt.data.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), &frame, |b, frame| {
            b.iter_batched(
                || { let mut e = EvrtckEncoder::new(w, h); e.encode(&base, 1); e },
                |mut enc| black_box(enc.encode(black_box(frame), 2)),
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_keyframes,
    bench_nop_static_frame,
    bench_pframes,
    bench_dirty_ratio_scan,
    bench_roundtrip,
    bench_pframe_sizes,
);
criterion_main!(benches);
