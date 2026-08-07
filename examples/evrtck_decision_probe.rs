//! EVRTCK decision probe.
//!
//! Run:
//!   cargo run --release --example evrtck_decision_probe -- --quick
//!   cargo run --release --example evrtck_decision_probe -- --iters 80
//!
//! This probe prints encode/decode/roundtrip quantiles and payload size for
//! scheduler decisions: when EVRTCK stays inside the frame budget and when
//! high-entropy updates should be handed to a hardware video codec.

use evertydesk_core::evrtck::{EvrtckDecoder, EvrtckEncoder, TILE_SIZE};
use std::hint::black_box;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
enum DirtyDistribution {
    Clustered,
    Scattered,
}

impl DirtyDistribution {
    fn as_str(self) -> &'static str {
        match self {
            Self::Clustered => "clustered",
            Self::Scattered => "scattered",
        }
    }
}

#[derive(Clone, Copy)]
enum DirtyEntropy {
    Invert,
    Noise,
}

impl DirtyEntropy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Invert => "invert",
            Self::Noise => "noise",
        }
    }
}

struct Scenario {
    name: &'static str,
    dirty_fraction: f32,
    distribution: DirtyDistribution,
    entropy: DirtyEntropy,
}

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

fn solid_frame(w: usize, h: usize, color: [u8; 4]) -> Vec<u8> {
    color.iter().cycle().take(w * h * 4).copied().collect()
}

fn dirty_tile_indices(
    w: usize,
    h: usize,
    dirty_fraction: f32,
    distribution: DirtyDistribution,
    seed: u64,
) -> Vec<usize> {
    let tiles_x = (w + TILE_SIZE - 1) / TILE_SIZE;
    let tiles_y = (h + TILE_SIZE - 1) / TILE_SIZE;
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

fn dirty_frame(
    base: &[u8],
    w: usize,
    h: usize,
    dirty_fraction: f32,
    distribution: DirtyDistribution,
    entropy: DirtyEntropy,
) -> Vec<u8> {
    let mut frame = base.to_vec();
    let tiles_x = (w + TILE_SIZE - 1) / TILE_SIZE;
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

fn percentile(sorted: &[Duration], pct: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = ((pct / 100.0) * ((sorted.len() - 1) as f64)).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn micros(d: Duration) -> f64 {
    d.as_secs_f64() * 1_000_000.0
}

fn measure_encode(base: &[u8], frame: &[u8], w: usize, h: usize, iters: usize) -> Vec<Duration> {
    let mut samples = Vec::with_capacity(iters);
    for _ in 0..iters {
        let mut enc = EvrtckEncoder::new(w, h);
        enc.encode(base, 1);
        let t0 = Instant::now();
        let pkt = enc.encode(black_box(frame), 2);
        black_box(pkt.data.len());
        samples.push(t0.elapsed());
    }
    samples.sort_unstable();
    samples
}

fn measure_decode(kf_data: &[u8], pframe_data: &[u8], iters: usize) -> Vec<Duration> {
    let mut samples = Vec::with_capacity(iters);
    for _ in 0..iters {
        let mut dec = EvrtckDecoder::new();
        dec.decode_wire(kf_data).unwrap();
        let t0 = Instant::now();
        let pixels = dec.decode_wire(black_box(pframe_data)).unwrap();
        black_box(pixels.len());
        samples.push(t0.elapsed());
    }
    samples.sort_unstable();
    samples
}

fn measure_roundtrip(base: &[u8], frame: &[u8], w: usize, h: usize, iters: usize) -> Vec<Duration> {
    let mut samples = Vec::with_capacity(iters);
    for _ in 0..iters {
        let mut enc = EvrtckEncoder::new(w, h);
        let kf = enc.encode(base, 1);
        let mut dec = EvrtckDecoder::new();
        dec.decode_wire(&kf.data).unwrap();

        let t0 = Instant::now();
        let pkt = enc.encode(black_box(frame), 2);
        let pixels = dec.decode_wire(&pkt.data).unwrap();
        black_box((pkt.data.len(), pixels.len()));
        samples.push(t0.elapsed());
    }
    samples.sort_unstable();
    samples
}

fn parse_iters() -> usize {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--quick") {
        return 12;
    }
    args.windows(2)
        .find_map(|pair| {
            if pair[0] == "--iters" {
                pair[1].parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(40)
        .max(3)
}

fn main() {
    let iters = parse_iters();
    let (w, h) = (1920usize, 1080usize);
    let raw_bytes = w * h * 4;
    let base = solid_frame(w, h, [30, 30, 30, 255]);
    let scenarios = [
        Scenario {
            name: "static",
            dirty_fraction: 0.00,
            distribution: DirtyDistribution::Clustered,
            entropy: DirtyEntropy::Invert,
        },
        Scenario {
            name: "typing_low_entropy",
            dirty_fraction: 0.05,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Invert,
        },
        Scenario {
            name: "ide_scroll_low_entropy",
            dirty_fraction: 0.15,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Invert,
        },
        Scenario {
            name: "browser_scroll_low_entropy",
            dirty_fraction: 0.50,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Invert,
        },
        Scenario {
            name: "video_like_low_entropy",
            dirty_fraction: 0.90,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Invert,
        },
        Scenario {
            name: "typing_noise",
            dirty_fraction: 0.05,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Noise,
        },
        Scenario {
            name: "ide_scroll_noise",
            dirty_fraction: 0.15,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Noise,
        },
        Scenario {
            name: "browser_scroll_noise",
            dirty_fraction: 0.50,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Noise,
        },
        Scenario {
            name: "video_like_noise",
            dirty_fraction: 0.90,
            distribution: DirtyDistribution::Scattered,
            entropy: DirtyEntropy::Noise,
        },
    ];

    println!(
        "EVRTCK decision probe; resolution={}x{}; tile={}x{}; raw_bytes={}; iterations={}",
        w, h, TILE_SIZE, TILE_SIZE, raw_bytes, iters
    );
    println!(
        "scenario,dirty_pct,distribution,entropy,payload_bytes,raw_to_payload,encode_p50_us,encode_p95_us,encode_p99_us,decode_p50_us,decode_p95_us,decode_p99_us,roundtrip_p50_us,roundtrip_p95_us,roundtrip_p99_us"
    );

    for scenario in scenarios {
        let frame = dirty_frame(
            &base,
            w,
            h,
            scenario.dirty_fraction,
            scenario.distribution,
            scenario.entropy,
        );

        let mut payload_encoder = EvrtckEncoder::new(w, h);
        let kf = payload_encoder.encode(&base, 1);
        let pframe = payload_encoder.encode(&frame, 2);
        let payload_bytes = pframe.data.len();
        let ratio = raw_bytes as f64 / payload_bytes.max(1) as f64;

        let encode = measure_encode(&base, &frame, w, h, iters);
        let decode = measure_decode(&kf.data, &pframe.data, iters);
        let roundtrip = measure_roundtrip(&base, &frame, w, h, iters);

        println!(
            "{},{:.0},{},{},{},{:.2},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1}",
            scenario.name,
            scenario.dirty_fraction * 100.0,
            scenario.distribution.as_str(),
            scenario.entropy.as_str(),
            payload_bytes,
            ratio,
            micros(percentile(&encode, 50.0)),
            micros(percentile(&encode, 95.0)),
            micros(percentile(&encode, 99.0)),
            micros(percentile(&decode, 50.0)),
            micros(percentile(&decode, 95.0)),
            micros(percentile(&decode, 99.0)),
            micros(percentile(&roundtrip, 50.0)),
            micros(percentile(&roundtrip, 95.0)),
            micros(percentile(&roundtrip, 99.0)),
        );
    }
}
