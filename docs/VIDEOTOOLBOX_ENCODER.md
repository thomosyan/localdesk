# VideoToolbox capture and Annex-B recording

## Current implementation

- Captures display frames through ScreenCaptureKit.
- Retains each captured IOSurface and sends it through a bounded queue to a dedicated worker.
- Encodes H.264 using VideoToolbox.
- Optionally writes encoded access units to an Annex-B `.h264` elementary stream.

## Record a test stream

On macOS, run:

```bash
MACOSX_DEPLOYMENT_TARGET=13.0 cargo run --release -p localdesk-macos-server -- \
  --width 1920 --height 1080 --fps 60 --seconds 15 --output capture.h264
```

The output is raw H.264 Annex-B. It does not contain container timestamps; the current encoder assigns synthetic frame-index timestamps at the configured frame rate. The capture pipeline does not yet preserve ScreenCaptureKit presentation timestamps.

Inspect and decode it with FFmpeg:

```bash
ffprobe -f h264 -show_streams capture.h264
ffmpeg -f h264 -framerate 60 -i capture.h264 -f null -
```

The `-framerate` value must match the configured capture/encoder rate for this test.

## Conversion details

- VideoToolbox returns AVCC length-prefixed NAL units.
- The writer reads the NAL length-field width from the sample's format description (1, 2, or 4 bytes).
- NAL units are emitted with four-byte Annex-B start codes.
- On sync samples, SPS and PPS are taken from the format description and prepended when absent from the access unit.
- The writer validates truncated prefixes, zero-length NALs, and lengths that exceed the remaining access-unit bytes.

## Current limitations

- No UDP/RTP transmission, packet reassembly, Windows H.264 decoding, or display rendering yet.
- The raw `.h264` file has no PTS/DTS metadata. A timestamped container or RTP layer is needed to preserve timing.
- The current frame timestamps are synthetic (`frame_index / fps`), not ScreenCaptureKit capture timestamps.
- A successful file write does not itself prove independent decoder compatibility; verify with FFmpeg on the captured file.
