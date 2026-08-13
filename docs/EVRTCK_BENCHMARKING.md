# EVRTCK production benchmarking

This is the reproducible benchmark path for EVRTCK software-path evidence.
Use it when publishing numbers or answering technical review questions.

## Quick smoke

```powershell
.\scripts\run-evrtck-prod-bench.ps1 -Quick -Iterations 40 -Warmup 5
```

This only verifies that the benchmark runner works and produces files. Do not
publish quick results as final performance evidence.

## Full run

```powershell
.\scripts\run-evrtck-prod-bench.ps1 -Iterations 300 -Warmup 30
```

Output is written to:

```text
reports/evrtck-prod/<timestamp>/
  metadata.json
  evrtck_prod_bench.csv
  evrtck_prod_bench.jsonl
  summary.md
  summary.json
```

`reports/` is ignored by git. Attach the generated directory separately when
sharing results.

`summary.md` is the human-facing decision map. It classifies every P-frame
scenario into one of:

- `evrtck_120fps_ok`;
- `evrtck_60fps_ok`;
- `scheduler_decision_needed`;
- `prefer_hardware_codec`.

Default thresholds:

- 60 FPS budget: 16.667 ms;
- 120 FPS budget: 8.333 ms;
- payload warning: 25% of raw BGRA.

You can regenerate only the summary from an existing report:

```powershell
.\scripts\summarize-evrtck-prod-bench.ps1 -ReportDir .\reports\evrtck-prod\<timestamp>
```

## Metadata captured

`metadata.json` records:

- git commit hash;
- branch;
- dirty working tree flag;
- build profile;
- command;
- frame resolution;
- iteration/warmup count;
- CPU model;
- physical cores and logical processors;
- max CPU clock reported by Windows;
- OS version/build;
- `rustc -Vv`;
- `cargo -V`.

If `working_tree_dirty` is `true`, say so when publishing results. For formal
numbers, commit the code first and rerun from a clean tree.

## Scenarios

Resolution defaults to 1920x1080 BGRA.

The runner currently covers:

- keyframe gradient, 100% dirty;
- static P-frame, 0% dirty;
- clustered invert dirty tiles at 5%, 15%, 50%, 90%;
- scattered invert dirty tiles at 5%, 15%, 50%, 90%;
- scattered noise dirty tiles at 5%, 15%, 50%, 90%.

Each P-frame scenario reports:

- encode time;
- decode time;
- encode+decode roundtrip time;
- output payload bytes;
- total tiles;
- dirty tiles;
- solid tiles;
- delta tiles;
- mean;
- p50;
- p95;
- p99;
- min/max.

## What exactly is measured

Keyframe encode:

- creates a fresh encoder outside the measured result setup;
- measures the first `encode_with_stats()` call for a gradient frame;
- includes full EVRTCK keyframe tile compression cost.

P-frame encode:

- creates an encoder and encodes the base frame before the timed section;
- measures only encoding the changed P-frame;
- this answers the steady-state desktop-stream question.
- realistic IDE/browser/terminal scenes use their own matching base frame, not
  a synthetic solid-color baseline.
- hinted realistic scroll scenes feed explicit EVRTCK copy rects plus dirty
  rects, matching the capture-metadata path expected from Desktop Duplication
  move rects.

Decode:

- creates a decoder and decodes the keyframe before the timed section;
- measures only decoding the P-frame payload;
- this isolates steady-state viewer decode cost.

Roundtrip:

- creates encoder/decoder and establishes base state before the timed section;
- measures P-frame encode plus P-frame decode;
- does not include network, encryption, UI painting, capture, or sleep/scheduler
  delay.
- validates before timing that every normal and hinted P-frame decodes exactly
  to the expected RGBA frame reconstructed from the BGRA capture input.

Payload bytes:

- are the EVRTCK wire payload bytes returned by the encoder;
- do not include EVRT UDP/TCP framing overhead;
- do include EVRTCK frame header, dirty map, tile indexes, tile modes, and tile
  compressed data.

## Current limitations

This runner measures EVRTCK software-path codec cost. It does not yet benchmark:

- hardware H.264/H.265 encode on the same scene;
- hardware decode latency;
- capture latency;
- EVRT network packetization/reassembly;
- real recorded IDE/browser/scroll traces.

Those are separate comparison harnesses. Do not claim this benchmark proves
EVRTCK beats hardware H.264/H.265 universally. The defensible claim is narrower:

> For software-path desktop deltas under these dirty-ratio and entropy
> conditions, EVRTCK encode/decode/roundtrip stays inside the measured
> per-frame budget, while keyframes are the expensive case that should be
> handled by scheduler policy and/or hardware.

## Example review answer

If asked what enters `roundtrip`, answer:

> Roundtrip is codec roundtrip only: P-frame encode plus P-frame decode after
> both encoder and decoder have already established the base frame. It excludes
> network, encryption, capture, UI presentation, and process scheduling delay.
> Payload size is reported separately for every scenario.

If asked why P-frame time may not grow linearly with dirty ratio, answer:

> EVRTCK has multiple paths: static tiles are represented by the dirty map,
> solid/invert-like dirty tiles can collapse very cheaply, and noisy scattered
> tiles are the stress case. That is why the production bench separates
> clustered invert, scattered invert, and scattered noise instead of reporting
> one generic dirty-ratio number.
