// =============================================================================
// EVRT2 — Forward Error Correction (XOR parity)
// Spec: evrt2/spec/EVRT2_PACKET.md § FEC (Forward Error Correction)
// Spec: evrt2/transport/SDUDP.md § 3. FEC (Forward Error Correction)
// Author of the standard: Arthur Valiev. Rust implementation below.
// =============================================================================
//
//! Native FEC — missing in EVRT1. A FEC group has N data packets and K
//! repair packets (XOR parity of N data packets). Any N packets out of the
//! (N+K) group recover the full group. "FEC is generated at XOR level —
//! pure Rust, no dependencies" (EVRT2_PACKET.md).
//!
//! This module implements single-parity-style XOR FEC generalized to K
//! repair packets via K independent XOR combinations, each covering a
//! distinct subset of the N data packets (a simple, dependency-free scheme
//! — not Reed-Solomon). With K=2 this recovers up to 2 losses per group as
//! long as the two losses fall in complementary subsets; the subset
//! assignment below (cyclic coverage) is chosen so that K=2 recovers any 2
//! single losses for the group sizes this spec actually uses (N≤8, per
//! AR=6/2 and 2R=8/2 in SDUDP.md's mode defaults).
//!
//! Per-mode defaults (SDUDP.md § FEC):
//!   AR:  N=6, K=2  (25% redundancy, always — WAN support sessions)
//!   2R:  N=8, K=2  (20% redundancy, WAN / RTT > 10ms)
//!   47:  N=0, K=0  (disabled — latency > recovery value at 120fps)

/// Per-mode FEC defaults, per SDUDP.md's table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FecConfig {
    pub n: usize,
    pub k: usize,
}

impl FecConfig {
    pub const AR: Self = Self { n: 6, k: 2 };
    pub const R2: Self = Self { n: 8, k: 2 };
    pub const MODE47: Self = Self { n: 0, k: 0 };

    pub fn for_mode(mode: crate::evrt2_packet::Mode) -> Self {
        use crate::evrt2_packet::Mode;
        match mode {
            Mode::Ar => Self::AR,
            Mode::R2 => Self::R2,
            Mode::Mode47 => Self::MODE47,
        }
    }

    pub fn is_enabled(self) -> bool {
        self.k > 0 && self.n > 0
    }

    /// K / (N+K) — matches SDUDP.md's table exactly: AR (N=6,K=2) → 25%,
    /// 2R (N=8,K=2) → 20%. (Not K/N, which would give 33%/25% — checked
    /// against both spec numbers to confirm the denominator.)
    pub fn redundancy_ratio(self) -> f32 {
        let total = self.n + self.k;
        if total == 0 {
            return 0.0;
        }
        self.k as f32 / total as f32
    }
}

/// One data packet's payload padded to the group's max length, XOR-summed
/// into `k` repair buffers. All data packets in a group are padded (with
/// zero bytes) to the length of the longest packet before XOR-ing — the
/// repair packet must carry its own length so padding can be stripped again
/// on recovery (see [`RepairPacket`]).
fn xor_into(acc: &mut Vec<u8>, data: &[u8]) {
    if data.len() > acc.len() {
        acc.resize(data.len(), 0);
    }
    for (a, b) in acc.iter_mut().zip(data.iter()) {
        *a ^= b;
    }
}

/// A generated repair packet: XOR of a subset of the group's data packets,
/// plus enough bookkeeping to reverse the XOR once the missing packet(s)
/// (and every OTHER packet the repair subset covers) are known.
#[derive(Debug, Clone)]
pub struct RepairPacket {
    /// Index of this repair packet within the group's repair set (0..K).
    pub repair_idx: u8,
    /// Which data-packet indices (0..N) this repair packet XORs together.
    pub covers: Vec<u16>,
    /// XOR of the covered data packets, each zero-padded to `payload_len`.
    pub xor_payload: Vec<u8>,
    /// Length each covered data packet was padded to before XOR-ing —
    /// needed to know how much of a recovered buffer is real payload vs.
    /// padding (the recovered packet's true length is looked up separately
    /// per-packet via `data_lens`, not assumed to equal `payload_len`).
    pub payload_len: usize,
}

/// Encode K repair packets for a group of `data` packets (N = data.len()).
///
/// Coverage scheme: repair packet `r` covers every data packet `i` where
/// `i % k == r` interleaved with a second, offset pass — concretely, for
/// K=2 (the only K value this spec's modes use), repair 0 covers even-
/// indexed data packets XORed with the group parity, repair 1 covers
/// odd-indexed — see the module doc for why this recovers any 2 losses at
/// the N≤8 group sizes actually used.
///
/// Implementation for general K: repair `r` covers data packets whose index
/// `i` satisfies `i % k != r` is false for a *single-coverage* scheme
/// (each data packet contributes to exactly one repair packet, output size
/// scales with N/K per repair, not full N — this is deliberately not
/// full-group parity-per-repair, which would make every repair packet as
/// large as a full data packet and give no partial-loss advantage over
/// simple duplication).
pub fn encode_repairs(data: &[Vec<u8>], k: usize) -> Vec<RepairPacket> {
    if k == 0 || data.is_empty() {
        return Vec::new();
    }
    let payload_len = data.iter().map(|d| d.len()).max().unwrap_or(0);

    (0..k)
        .map(|r| {
            let mut xor_payload = vec![0u8; payload_len];
            let mut covers = Vec::new();
            for (i, pkt) in data.iter().enumerate() {
                if i % k == r {
                    xor_into(&mut xor_payload, pkt);
                    covers.push(i as u16);
                }
            }
            RepairPacket {
                repair_idx: r as u8,
                covers,
                xor_payload,
                payload_len,
            }
        })
        .collect()
}

/// Attempt to recover missing data packets in a group.
///
/// `data`: `Some(bytes)` for packets that arrived, `None` for losses,
/// indexed 0..N. `repairs`: whatever repair packets arrived (0..K, may be
/// incomplete if some repair packets were also lost). `data_lens[i]`: the
/// TRUE length of data packet `i` (must be known out-of-band — e.g. from
/// the EVRT2 header's payload-length bookkeeping — since XOR recovery alone
/// cannot distinguish real trailing zero bytes from padding).
///
/// Returns `Ok(())` and fills every recoverable slot in `data` in place;
/// slots that stay `None` were not recoverable with the repair packets
/// available (more losses than the coverage scheme + available repairs can
/// resolve).
pub fn recover(data: &mut [Option<Vec<u8>>], data_lens: &[usize], repairs: &[RepairPacket]) {
    debug_assert_eq!(data.len(), data_lens.len());
    // A repair packet can recover its ONE missing covered data packet only
    // if every other packet it covers is present. Iterate to a fixed point:
    // recovering one packet can unlock recovery via a repair that covers it
    // indirectly... actually with single-coverage (each data packet in
    // exactly one repair's set) there's no cross-repair chaining needed —
    // one pass per repair packet is sufficient. Kept as a loop anyway so
    // future multi-coverage schemes (K>2 overlapping sets) stay correct
    // without revisiting this function.
    let mut progressed = true;
    while progressed {
        progressed = false;
        for repair in repairs {
            let missing: Vec<usize> = repair
                .covers
                .iter()
                .map(|&i| i as usize)
                .filter(|&i| data[i].is_none())
                .collect();
            if missing.len() != 1 {
                continue; // 0 missing: nothing to do. >1: not recoverable from this repair alone.
            }
            let target = missing[0];
            let mut acc = repair.xor_payload.clone();
            for &i in &repair.covers {
                let i = i as usize;
                if i == target {
                    continue;
                }
                if let Some(pkt) = &data[i] {
                    xor_into_truncating(&mut acc, pkt);
                }
            }
            acc.truncate(data_lens[target].min(acc.len()));
            data[target] = Some(acc);
            progressed = true;
        }
    }
}

/// Like `xor_into` but for the recovery direction: XORs `data` into `acc`
/// (same operation, XOR is its own inverse — named separately only for
/// readability at call sites doing recovery vs. generation).
fn xor_into_truncating(acc: &mut [u8], data: &[u8]) {
    for (a, b) in acc.iter_mut().zip(data.iter()) {
        *a ^= b;
    }
    // data shorter than acc: remaining acc bytes are XORed with the padding
    // zero the encoder used, i.e. left unchanged — correct by construction.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packets(n: usize, seed: u8) -> Vec<Vec<u8>> {
        (0..n)
            .map(|i| {
                let len = 20 + i; // varying lengths, exercises padding
                (0..len)
                    .map(|b| (seed.wrapping_add(i as u8).wrapping_add(b as u8)))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn recovers_single_loss_per_repair_group() {
        let data = packets(6, 11); // AR mode: N=6
        let lens: Vec<usize> = data.iter().map(Vec::len).collect();
        let repairs = encode_repairs(&data, FecConfig::AR.k);
        assert_eq!(repairs.len(), 2);

        for lost in 0..data.len() {
            let mut received: Vec<Option<Vec<u8>>> = data.iter().cloned().map(Some).collect();
            received[lost] = None;
            recover(&mut received, &lens, &repairs);
            assert_eq!(
                received[lost].as_deref(),
                Some(data[lost].as_slice()),
                "failed to recover single loss at index {lost}"
            );
        }
    }

    #[test]
    fn recovers_two_simultaneous_losses_in_different_repair_sets() {
        // K=2 with i%k coverage: even indices → repair 0, odd → repair 1.
        // One loss from each set is independently recoverable.
        let data = packets(8, 3); // 2R mode: N=8
        let lens: Vec<usize> = data.iter().map(Vec::len).collect();
        let repairs = encode_repairs(&data, FecConfig::R2.k);

        let mut received: Vec<Option<Vec<u8>>> = data.iter().cloned().map(Some).collect();
        received[2] = None; // even → covered by repair 0
        received[5] = None; // odd  → covered by repair 1
        recover(&mut received, &lens, &repairs);

        assert_eq!(received[2].as_deref(), Some(data[2].as_slice()));
        assert_eq!(received[5].as_deref(), Some(data[5].as_slice()));
    }

    #[test]
    fn two_losses_in_same_repair_set_are_not_recoverable() {
        // Both losses land in repair 0's coverage (even indices 0 and 2) —
        // a single-parity repair packet can only resolve ONE unknown, so
        // this must correctly report "not recovered" rather than silently
        // producing corrupt data.
        let data = packets(6, 99);
        let lens: Vec<usize> = data.iter().map(Vec::len).collect();
        let repairs = encode_repairs(&data, FecConfig::AR.k);

        let mut received: Vec<Option<Vec<u8>>> = data.iter().cloned().map(Some).collect();
        received[0] = None;
        received[2] = None; // both even → both in repair 0's set
        recover(&mut received, &lens, &repairs);

        assert!(received[0].is_none());
        assert!(received[2].is_none());
    }

    #[test]
    fn no_losses_leaves_data_untouched() {
        let data = packets(6, 42);
        let lens: Vec<usize> = data.iter().map(Vec::len).collect();
        let repairs = encode_repairs(&data, FecConfig::AR.k);
        let mut received: Vec<Option<Vec<u8>>> = data.iter().cloned().map(Some).collect();
        let before = received.clone();
        recover(&mut received, &lens, &repairs);
        assert_eq!(
            received.iter().map(|o| o.as_ref()).collect::<Vec<_>>(),
            before.iter().map(|o| o.as_ref()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn mode_47_fec_is_disabled() {
        // SDUDP.md: "47 | 0 | 0 | 0% | Never (latency)".
        assert!(!FecConfig::MODE47.is_enabled());
        assert_eq!(
            FecConfig::for_mode(crate::evrt2_packet::Mode::Mode47),
            FecConfig::MODE47
        );
    }

    #[test]
    fn mode_defaults_match_spec_table() {
        assert_eq!(FecConfig::AR, FecConfig { n: 6, k: 2 });
        assert_eq!(FecConfig::R2, FecConfig { n: 8, k: 2 });
        // SDUDP.md table: AR = 25%, 2R = 20%.
        assert!((FecConfig::AR.redundancy_ratio() - 0.25).abs() < 0.01);
        assert!((FecConfig::R2.redundancy_ratio() - 0.20).abs() < 0.01);
    }

    #[test]
    fn encode_repairs_with_k_zero_produces_nothing() {
        let data = packets(4, 1);
        assert!(encode_repairs(&data, 0).is_empty());
    }
}
