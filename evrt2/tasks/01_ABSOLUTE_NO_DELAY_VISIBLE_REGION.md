# EVRT2CKMAX — OR: ABSOLUTE NO DELAY IN THE VISIBLE REGION

**Task ID:** EVRT2CKMAX-TASK-01  
**Author:** Arthur Valiev  
**Status:** Specification → pending implementation  
**Depends on:** [`EVRT2CKMAX.md`](../codec/EVRT2CKMAX.md) — Perceptual Age Field,
Attention Map (object 1), Reconstruction Budget (object 3), Temporal APF

---

## Problem Statement

Everywhere else in this specification, age is a *soft* target:
`age_max(i)` is a function the system tries to satisfy, and the
Reconstruction Budget distributes resources proportionally to `P_i`.
Proportional allocation is correct for the whole frame — but it has
a failure mode: under heavy load, even the highest-priority region
can be starved a little, because "a little less than everyone else"
still degrades everyone together.

This task exists to close that gap for one specific region: the one
the viewer's attention is locked onto **right now**. For that region,
"a little degraded" is not acceptable. It is the region where delay
is felt most directly, and it is the region this entire standard was
built to protect first.

## The Absolute Guarantee

> **The Visible Region shall never exceed its age ceiling, even at
> the cost of every other region in the frame.**

This is not a priority. It is a **floor**, carved out of the
Reconstruction Budget before proportional allocation happens to
anything else.

## Definition: Visible Region

The Visible Region is the highest-priority contiguous area of the
current Attention Map — the region the Attention Priority Field
would already rank at the top, made explicit as its own object so it
can be given a hard guarantee instead of a soft one.

```
visible_region = {
    tiles : Attention Map tiles where P_i ≥ P_visible_threshold
    source: explicit client focus (cursor bounding box, active
            input target) when available, else the top-percentile
            tiles from the Attention Map itself
}

Default P_visible_threshold by mode:
  AR:  0.85   (cursor + immediate UI focus)
  2R:  0.80   (camera/action center)
  47:  0.75   (crosshair + confirmed target — wider, motion is faster)
```

The Visible Region is recomputed every frame. It typically covers a
small, bounded area — a few tiles around the cursor or crosshair, not
the whole screen. This is what makes an absolute guarantee affordable:
the floor is cheap because the region is small.

## Age Ceiling

```
age_ceiling(visible_region) by mode:
  AR:   12ms
  2R:   15ms
  47:    8ms

This is stricter than age_max(i) for the same priority elsewhere —
it is not derived from the age_max(i) formula, it is a hard constant
per mode, independent of network conditions.
```

## Mechanism

### 1. Budget floor, allocated first

The Reconstruction Budget (object 3) currently allocates
proportionally to `P_i` across all regions. This task changes the
allocation order:

```
1. Reserve: cost to deliver visible_region within age_ceiling
2. Allocate remaining budget proportionally to every other region
   by the existing P_i formula (unchanged)

if reserved_cost > total_budget:
    // Network cannot sustain even the floor — see Breach Handling
```

The floor is reserved even if it means every other region in the
frame receives zero budget for that tick. Periphery tiles missing an
update for a frame is acceptable. The visible region missing its
ceiling is not.

### 2. Packet scheduler preemption

In the SD-UDP packet scheduler ([`SDUDP.md`](../transport/SDUDP.md)),
visible-region slices are elevated above the existing IDR-first
ordering:

```
Send order (revised):
    1. Visible-region slices (new — highest priority, always first)
    2. IDR slices (existing)
    3. Remaining slices, priority order (existing)
    4. FEC repair (existing)
```

If mid-frame congestion is detected (via Transport Feedback pressure
signal) before the visible region has been fully sent, **all
remaining non-visible-region packets for this frame are dropped**,
not delayed. A dropped periphery tile is invisible next frame. A
delayed visible-region tile is felt now.

### 3. Wire signal

A new flag in the EVRT2 packet header
([`EVRT2_PACKET.md`](../spec/EVRT2_PACKET.md) Flags field, bit 8,
currently reserved):

```
Bit 8 | VISIBLE_REGION | This packet is part of the current Visible Region
```

The client's jitter buffer treats `VISIBLE_REGION` packets with
`buffer_depth = 0` — no buffering delay is applied, they are
decoded and rendered as soon as they arrive, ahead of buffer-depth
rules that apply to the rest of the frame.

## Breach Handling

If the age ceiling is missed anyway — network cannot sustain even
the floor — the system does not pretend it met the guarantee.

```
if actual_age(visible_region) > age_ceiling:
    emit DEGRADE_SIGNAL { region: visible_region, measured_age }
    client displays a visible degradation indicator
        (not a full stall — a discreet marker, mode-dependent)
    client Warp engine takes over more aggressively for this region
        (see Warp and Prediction, EVRT2CKMAX.md)
```

**This must not become an excuse to fabricate.** The
[Valiev Causal Integrity Principle](../codec/EVRT2CKMAX.md#valiev-causal-integrity-principle)
still applies without exception: Warp may extrapolate the visible
region's motion, it may never assert a server-confirmed outcome that
hasn't arrived. A missed ceiling is reported honestly, not hidden
behind a fabricated frame.

## Relationship to the Marginal Utility Test

This guarantee is the one deliberate exception to the marginal
utility test in the Law of Computational Opportunity. Everywhere
else, a resource is used only if it improves the objective function
on average. For the Visible Region, the rule is stricter: **whatever
resource is needed to hit the ceiling is used, even if it is not the
cheapest option, because this term of the objective function
(`P_i × age_i` for the highest `P_i`) dominates the sum.** The
marginal utility test still governs how the *rest* of the frame is
handled — it never governs the floor itself.

## Acceptance Criteria

1. Under synthetic network jitter injection (0–40ms P95, sustained
   10 minutes), Visible Region age stays within `age_ceiling` for
   **≥99.9%** of frames in each mode.
2. When the ceiling is breached, `DEGRADE_SIGNAL` fires within the
   same frame — no silent breaches.
3. Breach rate correlates only with network conditions exceeding the
   mode's floor bandwidth requirement — never with periphery frame
   complexity (proof that the floor is reserved *before*
   proportional allocation, not fighting for leftover budget).
4. No fabricated content appears in the Visible Region during Warp
   fallback — verified against the Causal Integrity Principle by
   diffing predicted vs. server-confirmed state after the fact.

## Implementation Milestones (Rust)

| Milestone | File | Description |
|-----------|------|-------------|
| M1 | `src/evrt_session.rs` | Compute `visible_region` from Attention Map each frame |
| M2 | `src/evrtck.rs` | Reorder tile encode/send to prioritize visible-region tiles first |
| M3 | `src/evrt.rs` | Add `VISIBLE_REGION` flag bit to packet header; scheduler preemption logic |
| M4 | `src/evrt_client.rs` | Jitter buffer bypass (`buffer_depth = 0`) for `VISIBLE_REGION` packets |
| M5 | `src/evrt.rs` | `DEGRADE_SIGNAL` packet type + client-side indicator hook |
| M6 | test harness | Jitter-injection test rig, automated acceptance-criteria check (#1–#3) |

M1–M2 can land on the existing EVRT1 transport without waiting for
the full EVRT2 protocol — the visible-region reordering benefit does
not require the new wire format, only the scheduling change.
M3–M5 require the EVRT2 packet header (new flag bit) and are natural
EVRT2-track work.

---

*EVRT2CKMAX-TASK-01. Arthur Valiev, 2026.*
