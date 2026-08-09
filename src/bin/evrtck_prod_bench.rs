use evertydesk_core::evrtck::{EvrtckDecoder, EvrtckEncoder, TILE_SIZE};
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
}

fn scenarios() -> &'static [Scenario] {
    &[
        Scenario {
            name: "static_0pct",
            dirty_ratio: 0.00,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
        },
        Scenario {
            name: "clustered_invert_5pct",
            dirty_ratio: 0.05,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
        },
        Scenario {
            name: "clustered_invert_15pct",
            dirty_ratio: 0.15,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
        },
        Scenario {
            name: "clustered_invert_50pct",
            dirty_ratio: 0.50,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
        },
        Scenario {
            name: "clustered_invert_90pct",
            dirty_ratio: 0.90,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
        },
        Scenario {
            name: "scattered_invert_5pct",
            dirty_ratio: 0.05,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Invert,
        },
        Scenario {
            name: "scattered_invert_15pct",
            dirty_ratio: 0.15,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Invert,
        },
        Scenario {
            name: "scattered_invert_50pct",
            dirty_ratio: 0.50,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Invert,
        },
        Scenario {
            name: "scattered_invert_90pct",
            dirty_ratio: 0.90,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Invert,
        },
        Scenario {
            name: "scattered_noise_5pct",
            dirty_ratio: 0.05,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Noise,
        },
        Scenario {
            name: "scattered_noise_15pct",
            dirty_ratio: 0.15,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Noise,
        },
        Scenario {
            name: "scattered_noise_50pct",
            dirty_ratio: 0.50,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Noise,
        },
        Scenario {
            name: "scattered_noise_90pct",
            dirty_ratio: 0.90,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Noise,
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
    frame: &[u8],
    scenario: Scenario,
) -> Vec<BenchRow> {
    let mut reference_encoder = EvrtckEncoder::new(config.width, config.height);
    let keyframe = reference_encoder.encode(base, 1);
    let (pframe, stats) = reference_encoder.encode_with_stats(frame, 2);
    let payload_bytes = pframe.data.len();

    let encode_samples = measure(config, || {
        let mut enc = EvrtckEncoder::new(config.width, config.height);
        enc.encode(black_box(base), 1);
        let started = Instant::now();
        let pkt = enc.encode_with_stats(black_box(frame), 2);
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
            "roundtrip",
            config,
            scenario.dirty_ratio,
            scenario.distribution,
            scenario.entropy,
            payload_bytes,
            stats,
            roundtrip_samples,
        ),
    ]
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
