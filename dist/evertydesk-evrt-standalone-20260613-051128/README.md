# EvertyDesk EVRT standalone package

This archive is an EVRT-focused source/documentation slice, not a full
EvertyDesk Lite build tree.

Included:

- `HABR_EVRT_ARTICLE.md` - long-form EVRT article draft.
- `EVRT_ROADMAP.md` - EVRT roadmap and current state.
- `WINDOWS_DEV_GUIDE.md` - Windows build/test notes for EVRT.
- `docs/video-codecs.md` and `docs/stream-stability-rules.md`.
- `src/evrt.rs` - EVRT packet protocol.
- `src/evrt_client.rs` - client-side UDP receiver, reassembly, feedback.
- `src/evrt_session.rs` - host-side UDP session, pacing, adaptive relief.
- `src/evrt_audio.rs` - EVRT audio path.
- `src/frame_queue.rs` - LatestAccessUnitQueue/adaptive jitter/reassembly logic.
- `src/video_pipeline.rs` - capture/encode/dispatch integration with TCP fallback and EVRT.
- Codec/capture support files used by the pipeline: `fsr.rs`, `capture.rs`,
  `colorconv.rs`, `nvenc.rs`, `nvenc_shim.cpp`, `mf_encode.rs`, `mf_video.rs`,
  `videotoolbox.rs`.

Notes:

- This package is intended for review, publication, architecture discussion,
  and extracting EVRT ideas.
- It will not compile standalone without the rest of the EvertyDesk Lite crate.
- For a full build, use the main repository root.
