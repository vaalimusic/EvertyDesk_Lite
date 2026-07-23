//! Standalone, runnable proof of EVRT2CKMAX-TASK-01 (visible-region priority).
//!
//! Run with:
//!   cargo run --example evrtck_focus_demo
//!
//! No GPU, no network, no phone required — this builds a synthetic frame,
//! dirties a handful of tiles scattered across it, and shows the actual byte
//! order EVRTCK v2 puts them on the wire with and without a focus point set.

use evertydesk_core::evrtck::{EvrtckDecoder, EvrtckEncoder};

const TILE: usize = 32;

fn dirty_one_pixel_in_tile(frame: &mut [u8], w: usize, idx: usize, tiles_x: usize, value: u8) {
    let tx = idx % tiles_x;
    let ty = idx / tiles_x;
    let px = tx * TILE + 3;
    let py = ty * TILE + 3;
    let off = (py * w + px) * 4;
    frame[off] = value;
}

/// Parse the wire stream and return tile_idx values in the order they appear.
fn wire_tile_order(data: &[u8]) -> Vec<u16> {
    let map_bytes = u16::from_le_bytes([data[18], data[19]]) as usize;
    let dirty_count: usize = data[20..20 + map_bytes]
        .iter()
        .map(|b| b.count_ones() as usize)
        .sum();
    let mut pos = 20 + map_bytes;
    let mut order = Vec::with_capacity(dirty_count);
    for _ in 0..dirty_count {
        let idx = u16::from_le_bytes([data[pos], data[pos + 1]]);
        order.push(idx);
        pos += 2;
        let mode = data[pos];
        pos += 1;
        match mode {
            1 => pos += 4, // MODE_SOLID: 4-byte color
            _ => {
                let len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
                pos += 4 + len;
            }
        }
    }
    order
}

fn main() {
    let (w, h) = (256, 128); // 8x4 = 32 tiles of 32x32, tiles_x = 8
    let tiles_x = 8;

    println!("EVRT2CKMAX-TASK-01 — Visible Region priority ordering demo");
    println!("Frame: {w}x{h} ({tiles_x}x4 = 32 tiles of {TILE}x{TILE})\n");

    // Scatter 6 dirty tiles across the frame, deliberately in an order that
    // does NOT match any interesting priority pattern on its own.
    let dirty_tile_idxs = [2usize, 7, 12, 18, 25, 30];
    println!("Dirty tiles (raster indices): {dirty_tile_idxs:?}");
    println!(
        "  → grid positions: {:?}\n",
        dirty_tile_idxs
            .iter()
            .map(|&i| (i % tiles_x, i / tiles_x))
            .collect::<Vec<_>>()
    );

    // ── Baseline: no focus, raster order ────────────────────────────────────
    let mut frame = vec![0u8; w * h * 4];
    let mut enc = EvrtckEncoder::new(w, h);
    enc.encode(&frame, 1); // keyframe baseline

    for &idx in &dirty_tile_idxs {
        dirty_one_pixel_in_tile(&mut frame, w, idx, tiles_x, 200);
    }
    let pkt_raster = enc.encode(&frame, 2);
    let order_raster = wire_tile_order(&pkt_raster.data);
    println!("WITHOUT focus (raster order):");
    println!("  wire order: {order_raster:?}\n");

    // ── With focus near tile 25 (grid pos (1,3)) ────────────────────────────
    let mut frame2 = vec![0u8; w * h * 4];
    let mut enc2 = EvrtckEncoder::new(w, h);
    let mut dec2 = EvrtckDecoder::new();
    dec2.decode(&enc2.encode(&frame2, 1)).unwrap();

    for &idx in &dirty_tile_idxs {
        dirty_one_pixel_in_tile(&mut frame2, w, idx, tiles_x, 200);
    }
    let focus_tile = (1usize, 3usize); // tile 25's own grid position
    enc2.set_focus_pixel((focus_tile.0 * TILE + 1) as u32, (focus_tile.1 * TILE + 1) as u32);
    let pkt_focus = enc2.encode(&frame2, 2);
    let order_focus = wire_tile_order(&pkt_focus.data);
    println!("WITH focus at tile {focus_tile:?} (= tile 25's position):");
    println!("  wire order: {order_focus:?}");
    println!(
        "  → nearest tile (25) moved to position {} (was position {} in raster order)\n",
        order_focus.iter().position(|&i| i == 25).unwrap(),
        order_raster.iter().position(|&i| i == 25).unwrap(),
    );

    // ── Prove it still decodes correctly despite the reordering ────────────
    let decoded = dec2.decode(&pkt_focus).unwrap();
    let expected: Vec<u8> = frame2.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
    let correct = decoded == expected.as_slice();
    println!(
        "Round-trip correctness with priority-ordered wire stream: {}",
        if correct { "PASS ✓" } else { "FAIL ✗" }
    );
    println!(
        "\nPacket sizes — raster: {} bytes, focus-ordered: {} bytes (same data, different order, same cost)",
        pkt_raster.data.len(),
        pkt_focus.data.len()
    );

    assert!(correct, "demo invariant violated — this should never happen");
}
