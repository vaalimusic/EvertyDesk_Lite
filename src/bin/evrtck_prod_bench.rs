use evertydesk_core::evrtck::{DirtyRect, EvrtckDecoder, EvrtckEncoder, TILE_SIZE};
use serde::Serialize;
use std::fs::{self, File};
use std::hint::black_box;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum DirtyDistribution {
    Clustered,
    Scattered,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum DirtyEntropy {
    Invert,
    Noise,
}

#[derive(Debug)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

#[derive(Debug, Clone)]
struct Config {
    width: usize,
    height: usize,
    iterations: usize,
    warmup: usize,
    out_dir: PathBuf,
}

impl Config {
    fn parse() -> Result<Self, String> {
        let mut width = 1920usize;
        let mut height = 1080usize;
        let mut iterations = 300usize;
        let mut warmup = 30usize;
        let mut out_dir: Option<PathBuf> = None;
        let mut quick = false;

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--width" => width = parse_next(&mut args, "--width")?,
                "--height" => height = parse_next(&mut args, "--height")?,
                "--iterations" => iterations = parse_next(&mut args, "--iterations")?,
                "--warmup" => warmup = parse_next(&mut args, "--warmup")?,
                "--out" => out_dir = Some(PathBuf::from(next_arg(&mut args, "--out")?)),
                "--quick" => quick = true,
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }

        if quick {
            iterations = iterations.min(40);
            warmup = warmup.min(5);
        }
        if width == 0 || height == 0 {
            return Err("width/height must be positive".to_owned());
        }
        if iterations == 0 {
            return Err("iterations must be positive".to_owned());
        }

        let out_dir = out_dir.unwrap_or_else(default_out_dir);
        Ok(Self {
            width,
            height,
            iterations,
            warmup,
            out_dir,
        })
    }
}

#[derive(Debug, Serialize)]
struct BenchRow {
    scenario: String,
    operation: String,
    width: usize,
    height: usize,
    dirty_ratio_target: f32,
    dirty_distribution: DirtyDistribution,
    dirty_entropy: DirtyEntropy,
    iterations: usize,
    warmup: usize,
    raw_bytes: usize,
    payload_bytes: usize,
    dirty_tiles: u32,
    total_tiles: u32,
    solid_tiles: u32,
    delta_tiles: u32,
    mean_us: f64,
    p50_us: f64,
    p95_us: f64,
    p99_us: f64,
    min_us: f64,
    max_us: f64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = match Config::parse() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            eprintln!();
            print_help();
            std::process::exit(2);
        }
    };

    fs::create_dir_all(&config.out_dir)?;
    let csv_path = config.out_dir.join("evrtck_prod_bench.csv");
    let jsonl_path = config.out_dir.join("evrtck_prod_bench.jsonl");
    let mut csv = BufWriter::new(File::create(&csv_path)?);
    let mut jsonl = BufWriter::new(File::create(&jsonl_path)?);

    writeln!(
        csv,
        "scenario,operation,width,height,dirty_ratio_target,dirty_distribution,dirty_entropy,iterations,warmup,raw_bytes,payload_bytes,dirty_tiles,total_tiles,solid_tiles,delta_tiles,mean_us,p50_us,p95_us,p99_us,min_us,max_us"
    )?;

    let base = solid_frame(config.width, config.height, [30, 30, 30, 255]);
    let keyframe = gradient_frame(config.width, config.height);

    let mut rows = Vec::new();
    rows.extend(run_keyframe_scenario(&config, &keyframe));
    for scenario in scenarios() {
        let frame = dirty_frame(
            &base,
            config.width,
            config.height,
            scenario.dirty_ratio,
            scenario.distribution,
            scenario.entropy,
        );
        rows.extend(run_pframe_scenario(&config, &base, &frame, *scenario));
    }

    for row in &rows {
        writeln!(
            csv,
            "{},{},{},{},{:.4},{:?},{:?},{},{},{},{},{},{},{},{},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3}",
            row.scenario,
            row.operation,
            row.width,
            row.height,
            row.dirty_ratio_target,
            row.dirty_distribution,
            row.dirty_entropy,
            row.iterations,
            row.warmup,
            row.raw_bytes,
            row.payload_bytes,
            row.dirty_tiles,
            row.total_tiles,
            row.solid_tiles,
            row.delta_tiles,
            row.mean_us,
            row.p50_us,
            row.p95_us,
            row.p99_us,
            row.min_us,
            row.max_us
        )?;
        writeln!(jsonl, "{}", serde_json::to_string(row)?)?;
    }

    csv.flush()?;
    jsonl.flush()?;

    println!("EVRTCK production bench complete");
    println!("csv={}", csv_path.display());
    println!("jsonl={}", jsonl_path.display());
    Ok(())
}

#[derive(Clone, Copy)]
struct Scenario {
    name: &'static str,
    dirty_ratio: f32,
    distribution: DirtyDistribution,
    entropy: DirtyEntropy,
    scene: SceneKind,
}

#[derive(Clone, Copy)]
enum SceneKind {
    SyntheticDirty,
    IdeTyping,
    BrowserScroll,
    TerminalScroll,
}

fn scenarios() -> &'static [Scenario] {
    &[
        Scenario {
            name: "static_0pct",
            dirty_ratio: 0.00,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "clustered_invert_5pct",
            dirty_ratio: 0.05,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "clustered_invert_15pct",
            dirty_ratio: 0.15,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "clustered_invert_50pct",
            dirty_ratio: 0.50,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "clustered_invert_90pct",
            dirty_ratio: 0.90,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "scattered_invert_5pct",
            dirty_ratio: 0.05,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "scattered_invert_15pct",
            dirty_ratio: 0.15,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "scattered_invert_50pct",
            dirty_ratio: 0.50,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "scattered_invert_90pct",
            dirty_ratio: 0.90,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "scattered_noise_5pct",
            dirty_ratio: 0.05,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Noise,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "scattered_noise_15pct",
            dirty_ratio: 0.15,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Noise,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "scattered_noise_50pct",
            dirty_ratio: 0.50,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Noise,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "scattered_noise_90pct",
            dirty_ratio: 0.90,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Noise,
            scene: SceneKind::SyntheticDirty,
        },
        Scenario {
            name: "ide_typing_realistic",
            dirty_ratio: 0.01,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::IdeTyping,
        },
        Scenario {
            name: "browser_scroll_realistic",
            dirty_ratio: 0.08,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::BrowserScroll,
        },
        Scenario {
            name: "terminal_scroll_realistic",
            dirty_ratio: 0.04,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
            scene: SceneKind::TerminalScroll,
        },
    ]
}

fn run_keyframe_scenario(config: &Config, frame: &[u8]) -> Vec<BenchRow> {
    let samples = measure(config, || {
        let mut enc = EvrtckEncoder::new(config.width, config.height);
        let started = Instant::now();
        let pkt = enc.encode_with_stats(black_box(frame), 1);
        let elapsed = started.elapsed();
        black_box(pkt.0.data.len());
        elapsed
    });

    let mut enc = EvrtckEncoder::new(config.width, config.height);
    let (pkt, stats) = enc.encode_with_stats(frame, 1);
    vec![row(
        "keyframe_gradient",
        "encode",
        config,
        1.0,
        DirtyDistribution::Clustered,
        DirtyEntropy::Invert,
        pkt.data.len(),
        stats,
        samples,
    )]
}

fn run_pframe_scenario(
    config: &Config,
    base: &[u8],
    synthetic_frame: &[u8],
    scenario: Scenario,
) -> Vec<BenchRow> {
    let scene = make_scene(config, base, synthetic_frame, scenario);
    let frame = scene.frame.as_slice();
    let dirty_rects = scene.dirty_rects.as_slice();

    let mut reference_encoder = EvrtckEncoder::new(config.width, config.height);
    let keyframe = reference_encoder.encode(base, 1);
    let (pframe, stats) = reference_encoder.encode_with_stats(frame, 2);
    let payload_bytes = pframe.data.len();

    let mut hinted_reference_encoder = EvrtckEncoder::new(config.width, config.height);
    hinted_reference_encoder.encode(base, 1);
    let (hinted_pframe, hinted_stats) =
        hinted_reference_encoder.encode_with_capture_hints(frame, 2, &[], dirty_rects);
    let hinted_payload_bytes = hinted_pframe.data.len();

    let encode_samples = measure(config, || {
        let mut enc = EvrtckEncoder::new(config.width, config.height);
        enc.encode(black_box(base), 1);
        let started = Instant::now();
        let pkt = enc.encode_with_stats(black_box(frame), 2);
        let elapsed = started.elapsed();
        black_box(pkt.0.data.len());
        elapsed
    });

    let hinted_encode_samples = measure(config, || {
        let mut enc = EvrtckEncoder::new(config.width, config.height);
        enc.encode(black_box(base), 1);
        let started = Instant::now();
        let pkt = enc.encode_with_capture_hints(black_box(frame), 2, &[], black_box(dirty_rects));
        let elapsed = started.elapsed();
        black_box(pkt.0.data.len());
        elapsed
    });

    let decode_samples = measure(config, || {
        let mut dec = EvrtckDecoder::new();
        dec.decode_wire(black_box(&keyframe.data)).unwrap();
        let started = Instant::now();
        let pixels = dec.decode_wire(black_box(&pframe.data)).unwrap();
        let elapsed = started.elapsed();
        black_box(pixels.len());
        elapsed
    });

    let hinted_decode_samples = measure(config, || {
        let mut dec = EvrtckDecoder::new();
        dec.decode_wire(black_box(&keyframe.data)).unwrap();
        let started = Instant::now();
        let pixels = dec.decode_wire(black_box(&hinted_pframe.data)).unwrap();
        let elapsed = started.elapsed();
        black_box(pixels.len());
        elapsed
    });

    let roundtrip_samples = measure(config, || {
        let mut enc = EvrtckEncoder::new(config.width, config.height);
        let kf = enc.encode(black_box(base), 1);
        let mut dec = EvrtckDecoder::new();
        dec.decode_wire(black_box(&kf.data)).unwrap();
        let started = Instant::now();
        let pkt = enc.encode(black_box(frame), 2);
        let pixels = dec.decode_wire(black_box(&pkt.data)).unwrap();
        let elapsed = started.elapsed();
        black_box((pkt.data.len(), pixels.len()));
        elapsed
    });

    let hinted_roundtrip_samples = measure(config, || {
        let mut enc = EvrtckEncoder::new(config.width, config.height);
        let kf = enc.encode(black_box(base), 1);
        let mut dec = EvrtckDecoder::new();
        dec.decode_wire(black_box(&kf.data)).unwrap();
        let started = Instant::now();
        let pkt = enc.encode_with_capture_hints(black_box(frame), 2, &[], black_box(dirty_rects));
        let pixels = dec.decode_wire(black_box(&pkt.0.data)).unwrap();
        let elapsed = started.elapsed();
        black_box((pkt.0.data.len(), pixels.len()));
        elapsed
    });

    vec![
        row(
            scenario.name,
            "encode",
            config,
            scenario.dirty_ratio,
            scenario.distribution,
            scenario.entropy,
            payload_bytes,
            stats.clone(),
            encode_samples,
        ),
        row(
            scenario.name,
            "encode_hinted",
            config,
            scenario.dirty_ratio,
            scenario.distribution,
            scenario.entropy,
            hinted_payload_bytes,
            hinted_stats.clone(),
            hinted_encode_samples,
        ),
        row(
            scenario.name,
            "decode",
            config,
            scenario.dirty_ratio,
            scenario.distribution,
            scenario.entropy,
            payload_bytes,
            stats.clone(),
            decode_samples,
        ),
        row(
            scenario.name,
            "decode_hinted",
            config,
            scenario.dirty_ratio,
            scenario.distribution,
            scenario.entropy,
            hinted_payload_bytes,
            hinted_stats.clone(),
            hinted_decode_samples,
        ),
        row(
            scenario.name,
            "roundtrip",
            config,
            scenario.dirty_ratio,
            scenario.distribution,
            scenario.entropy,
            payload_bytes,
            stats,
            roundtrip_samples,
        ),
        row(
            scenario.name,
            "roundtrip_hinted",
            config,
            scenario.dirty_ratio,
            scenario.distribution,
            scenario.entropy,
            hinted_payload_bytes,
            hinted_stats,
            hinted_roundtrip_samples,
        ),
    ]
}

struct Scene {
    frame: Vec<u8>,
    dirty_rects: Vec<DirtyRect>,
}

fn make_scene(config: &Config, base: &[u8], synthetic_frame: &[u8], scenario: Scenario) -> Scene {
    match scenario.scene {
        SceneKind::SyntheticDirty => Scene {
            frame: synthetic_frame.to_vec(),
            dirty_rects: dirty_rects_for_tiles(
                config.width,
                config.height,
                &dirty_tile_indices(
                    config.width,
                    config.height,
                    scenario.dirty_ratio,
                    scenario.distribution,
                    0x4556_5254_434b,
                ),
            ),
        },
        SceneKind::IdeTyping => ide_typing_scene(config.width, config.height),
        SceneKind::BrowserScroll => browser_scroll_scene(config.width, config.height),
        SceneKind::TerminalScroll => terminal_scroll_scene(base, config.width, config.height),
    }
}

fn measure<F>(config: &Config, mut f: F) -> Vec<Duration>
where
    F: FnMut() -> Duration,
{
    for _ in 0..config.warmup {
        black_box(f());
    }

    let mut samples = Vec::with_capacity(config.iterations);
    for _ in 0..config.iterations {
        samples.push(f());
    }
    samples
}

fn row(
    scenario: &str,
    operation: &str,
    config: &Config,
    dirty_ratio: f32,
    distribution: DirtyDistribution,
    entropy: DirtyEntropy,
    payload_bytes: usize,
    stats: evertydesk_core::evrtck::FrameStats,
    samples: Vec<Duration>,
) -> BenchRow {
    let summary = summarize(&samples);
    BenchRow {
        scenario: scenario.to_owned(),
        operation: operation.to_owned(),
        width: config.width,
        height: config.height,
        dirty_ratio_target: dirty_ratio,
        dirty_distribution: distribution,
        dirty_entropy: entropy,
        iterations: config.iterations,
        warmup: config.warmup,
        raw_bytes: config.width * config.height * 4,
        payload_bytes,
        dirty_tiles: stats.dirty_tiles,
        total_tiles: stats.total_tiles,
        solid_tiles: stats.solid_tiles,
        delta_tiles: stats.delta_tiles,
        mean_us: summary.mean_us,
        p50_us: summary.p50_us,
        p95_us: summary.p95_us,
        p99_us: summary.p99_us,
        min_us: summary.min_us,
        max_us: summary.max_us,
    }
}

struct Summary {
    mean_us: f64,
    p50_us: f64,
    p95_us: f64,
    p99_us: f64,
    min_us: f64,
    max_us: f64,
}

fn summarize(samples: &[Duration]) -> Summary {
    let mut micros: Vec<f64> = samples
        .iter()
        .map(|duration| duration.as_secs_f64() * 1_000_000.0)
        .collect();
    micros.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mean_us = micros.iter().sum::<f64>() / micros.len().max(1) as f64;
    Summary {
        mean_us,
        p50_us: percentile(&micros, 50.0),
        p95_us: percentile(&micros, 95.0),
        p99_us: percentile(&micros, 99.0),
        min_us: *micros.first().unwrap_or(&0.0),
        max_us: *micros.last().unwrap_or(&0.0),
    }
}

fn percentile(sorted: &[f64], percentile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((percentile / 100.0) * (sorted.len().saturating_sub(1)) as f64).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn solid_frame(w: usize, h: usize, color: [u8; 4]) -> Vec<u8> {
    color.iter().cycle().take(w * h * 4).copied().collect()
}

fn gradient_frame(w: usize, h: usize) -> Vec<u8> {
    (0..h)
        .flat_map(|y| {
            (0..w).flat_map(move |x| {
                let r = ((x * 255) / w) as u8;
                let g = ((y * 255) / h) as u8;
                let b = (((x + y) * 255) / (w + h)) as u8;
                [b, g, r, 255u8]
            })
        })
        .collect()
}

fn dirty_frame(
    base: &[u8],
    w: usize,
    h: usize,
    dirty_fraction: f32,
    distribution: DirtyDistribution,
    entropy: DirtyEntropy,
) -> Vec<u8> {
    let mut frame = base.to_vec();
    let tiles_x = w.div_ceil(TILE_SIZE);
    let dirty_tiles = dirty_tile_indices(w, h, dirty_fraction, distribution, 0x4556_5254_434b);
    let mut rng = SplitMix64::new(0x4556_5254_434b_0001);

    for tile_idx in dirty_tiles {
        let tx = tile_idx % tiles_x;
        let ty = tile_idx / tiles_x;
        let x0 = tx * TILE_SIZE;
        let y0 = ty * TILE_SIZE;
        let x1 = (x0 + TILE_SIZE).min(w);
        let y1 = (y0 + TILE_SIZE).min(h);
        for y in y0..y1 {
            for x in x0..x1 {
                let off = (y * w + x) * 4;
                match entropy {
                    DirtyEntropy::Invert => {
                        frame[off] = 255 - base[off];
                        frame[off + 1] = 255 - base[off + 1];
                        frame[off + 2] = 255 - base[off + 2];
                    }
                    DirtyEntropy::Noise => {
                        let noise = rng.next_u64();
                        frame[off] = noise as u8;
                        frame[off + 1] = (noise >> 8) as u8;
                        frame[off + 2] = (noise >> 16) as u8;
                    }
                }
            }
        }
    }
    frame
}

fn dirty_tile_indices(
    w: usize,
    h: usize,
    dirty_fraction: f32,
    distribution: DirtyDistribution,
    seed: u64,
) -> Vec<usize> {
    let tiles_x = w.div_ceil(TILE_SIZE);
    let tiles_y = h.div_ceil(TILE_SIZE);
    let total_tiles = tiles_x * tiles_y;
    let dirty_count = ((total_tiles as f32) * dirty_fraction).round() as usize;
    let dirty_count = dirty_count.min(total_tiles);

    match distribution {
        DirtyDistribution::Clustered => (0..dirty_count).collect(),
        DirtyDistribution::Scattered => {
            let mut rng = SplitMix64::new(seed);
            let mut tiles: Vec<usize> = (0..total_tiles).collect();
            for i in (1..tiles.len()).rev() {
                let j = (rng.next_u64() as usize) % (i + 1);
                tiles.swap(i, j);
            }
            tiles.truncate(dirty_count);
            tiles.sort_unstable();
            tiles
        }
    }
}

fn dirty_rects_for_tiles(w: usize, h: usize, tile_indices: &[usize]) -> Vec<DirtyRect> {
    let tiles_x = w.div_ceil(TILE_SIZE);
    tile_indices
        .iter()
        .copied()
        .map(|tile_idx| {
            let tx = tile_idx % tiles_x;
            let ty = tile_idx / tiles_x;
            let left = tx * TILE_SIZE;
            let top = ty * TILE_SIZE;
            DirtyRect {
                left: left as u32,
                top: top as u32,
                right: (left + TILE_SIZE).min(w) as u32,
                bottom: (top + TILE_SIZE).min(h) as u32,
            }
        })
        .collect()
}

fn ide_typing_scene(w: usize, h: usize) -> Scene {
    let mut frame = ide_base_frame(w, h);
    let editor_left = (w / 5).min(360);
    let editor_top = (h / 8).min(140);
    let line_h = 22usize;
    let caret_line = 11usize;
    let y0 = (editor_top + caret_line * line_h).min(h.saturating_sub(1));
    let x0 = (editor_left + 64).min(w.saturating_sub(1));
    let text_w = 320usize.min(w.saturating_sub(x0));
    let text_h = 18usize.min(h.saturating_sub(y0));

    fill_rect(&mut frame, w, h, x0, y0, text_w, text_h, [54, 61, 73, 255]);
    for i in 0..28 {
        let bar_x = x0 + i * 10;
        if bar_x + 6 >= w {
            break;
        }
        let color = match i % 5 {
            0 => [97, 175, 239, 255],
            1 => [152, 195, 121, 255],
            2 => [224, 108, 117, 255],
            _ => [210, 210, 210, 255],
        };
        fill_rect(&mut frame, w, h, bar_x, y0 + 4, 6, 10, color);
    }
    fill_rect(
        &mut frame,
        w,
        h,
        x0 + text_w + 8,
        y0 + 1,
        2,
        text_h,
        [240, 240, 240, 255],
    );

    Scene {
        frame,
        dirty_rects: vec![DirtyRect {
            left: x0 as u32,
            top: y0 as u32,
            right: (x0 + text_w + 12).min(w) as u32,
            bottom: (y0 + text_h).min(h) as u32,
        }],
    }
}

fn browser_scroll_scene(w: usize, h: usize) -> Scene {
    let mut frame = browser_base_frame(w, h, 0);
    let scroll_px = 96usize.min(h / 4).max(1);
    let old = browser_base_frame(w, h, 0);
    let newer = browser_base_frame(w, h, scroll_px);
    let content_top = (h / 10).max(80).min(h);
    let content_bottom = h.saturating_sub(24);
    if content_bottom > content_top + scroll_px {
        for y in content_top..(content_bottom - scroll_px) {
            let src = ((y + scroll_px) * w) * 4;
            let dst = (y * w) * 4;
            frame[dst..dst + w * 4].copy_from_slice(&old[src..src + w * 4]);
        }
    }
    let dirty_top = content_bottom.saturating_sub(scroll_px);
    let bytes_start = dirty_top * w * 4;
    frame[bytes_start..content_bottom * w * 4]
        .copy_from_slice(&newer[bytes_start..content_bottom * w * 4]);

    Scene {
        frame,
        dirty_rects: vec![DirtyRect {
            left: 0,
            top: dirty_top as u32,
            right: w as u32,
            bottom: content_bottom as u32,
        }],
    }
}

fn terminal_scroll_scene(base: &[u8], w: usize, h: usize) -> Scene {
    let mut frame = base.to_vec();
    let scroll_px = 32usize.min(h / 8).max(1);
    if h > scroll_px {
        for y in 0..(h - scroll_px) {
            let src = (y + scroll_px) * w * 4;
            let dst = y * w * 4;
            frame.copy_within(src..src + w * 4, dst);
        }
    }
    let y0 = h.saturating_sub(scroll_px);
    fill_rect(&mut frame, w, h, 0, y0, w, scroll_px, [18, 18, 18, 255]);
    for row in 0..(scroll_px / 8).max(1) {
        let y = y0 + row * 10 + 4;
        for col in 0..80usize {
            let x = 16 + col * 9;
            if x + 6 >= w || y + 2 >= h {
                break;
            }
            fill_rect(&mut frame, w, h, x, y, 6, 2, [84, 255, 112, 255]);
        }
    }

    Scene {
        frame,
        dirty_rects: vec![DirtyRect {
            left: 0,
            top: y0 as u32,
            right: w as u32,
            bottom: h as u32,
        }],
    }
}

fn ide_base_frame(w: usize, h: usize) -> Vec<u8> {
    let mut frame = solid_frame(w, h, [33, 37, 43, 255]);
    let sidebar_w = (w / 6).min(300);
    fill_rect(&mut frame, w, h, 0, 0, sidebar_w, h, [26, 29, 35, 255]);
    for i in 0..36usize {
        let y = 32 + i * 24;
        if y + 10 >= h {
            break;
        }
        let indent = (i % 4) * 14;
        fill_rect(
            &mut frame,
            w,
            h,
            sidebar_w + 40 + indent,
            y,
            180 + (i % 7) * 18,
            3,
            [92, 99, 112, 255],
        );
    }
    frame
}

fn browser_base_frame(w: usize, h: usize, offset: usize) -> Vec<u8> {
    let mut frame = solid_frame(w, h, [245, 247, 250, 255]);
    let toolbar_h = (h / 12).max(56).min(h);
    fill_rect(&mut frame, w, h, 0, 0, w, toolbar_h, [232, 235, 240, 255]);
    fill_rect(
        &mut frame,
        w,
        h,
        72.min(w),
        16.min(h),
        w.saturating_sub(144),
        28.min(h),
        [255, 255, 255, 255],
    );
    let card_w = (w / 3).max(220);
    for i in 0..24usize {
        let y = toolbar_h + 24 + i * 78usize.saturating_sub(offset % 78);
        if y >= h {
            continue;
        }
        let x = 48 + (i % 2) * (card_w + 24);
        fill_rect(
            &mut frame,
            w,
            h,
            x,
            y,
            card_w.min(w.saturating_sub(x)),
            54,
            [255, 255, 255, 255],
        );
        fill_rect(&mut frame, w, h, x + 18, y + 14, 160, 6, [80, 92, 110, 255]);
        fill_rect(
            &mut frame,
            w,
            h,
            x + 18,
            y + 30,
            card_w.saturating_sub(54),
            4,
            [180, 188, 202, 255],
        );
    }
    frame
}

fn fill_rect(
    frame: &mut [u8],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    rw: usize,
    rh: usize,
    bgra: [u8; 4],
) {
    let x1 = (x + rw).min(w);
    let y1 = (y + rh).min(h);
    for py in y.min(h)..y1 {
        for px in x.min(w)..x1 {
            let off = (py * w + px) * 4;
            frame[off..off + 4].copy_from_slice(&bgra);
        }
    }
}

fn default_out_dir() -> PathBuf {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    PathBuf::from("reports")
        .join("evrtck-prod")
        .join(seconds.to_string())
}

fn parse_next<T>(args: &mut impl Iterator<Item = String>, name: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    let value = next_arg(args, name)?;
    value
        .parse()
        .map_err(|_| format!("invalid value for {name}: {value}"))
}

fn next_arg(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("missing value after {name}"))
}

fn print_help() {
    eprintln!(
        "Usage: cargo run --release --bin evrtck_prod_bench -- [options]\n\
         \n\
         Options:\n\
           --width N          frame width, default 1920\n\
           --height N         frame height, default 1080\n\
           --iterations N     measured iterations per operation, default 300\n\
           --warmup N         warmup iterations per operation, default 30\n\
           --quick            cap at 40 iterations / 5 warmup\n\
           --out PATH         output directory, default reports/evrtck-prod/<unix-seconds>\n"
    );
}
