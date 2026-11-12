# VideoToolbox encoder milestone

This branch adds the first LocalDesk H.264 encoder wrapper on macOS.

## Current implementation

- Uses Apple's VideoToolbox through the Rust `videotoolbox` crate.
- Accepts an `IOSurface` directly, matching the IOSurface-backed frames delivered by ScreenCaptureKit.
- Configures real-time H.264 encoding, target bitrate, expected frame rate, and maximum keyframe interval.
- Returns the encoder's `EncodedFrame`.

## Important limitations

- The encoder wrapper is not connected to the ScreenCaptureKit callback yet.
- Encoding is synchronous in this wrapper. Call it from a dedicated worker, never directly from the capture callback.
- `EncodedFrame::data` is AVCC length-prefixed H.264 NAL data, not Annex-B. It is not yet a standalone `.h264` elementary stream.
- SPS/PPS are available through the encoded sample buffer's format description; parameter-set extraction and AVCC-to-Annex-B conversion are a follow-up task.
- No UDP/RTP transmission or Windows decoding is added in this milestone.

## Local validation

On an Apple Silicon Mac with Xcode installed:

```bash
cargo fmt --all
cargo check --workspace
cargo test --workspace
```

Then the next step is a live capture-to-encoder smoke test on macOS. A successful `cargo check` only proves the Rust/API integration compiles; it does not prove that the local VideoToolbox hardware encoder accepts the captured pixel format at runtime.
