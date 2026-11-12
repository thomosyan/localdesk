# Video pipeline: audit and implementation plan

## Audited repository state

This branch was created from `main` for the video pipeline work.

Current implementation:

- macOS: ScreenCaptureKit captures display frames and counts them. It does not yet hand frames to an encoder.
- macOS pixel format: capture is requested as `YCbCr_420v` (bi-planar 4:2:0), which is a reasonable input format for hardware H.264 encoding.
- Protocol: RTP fixed-header encode/decode and H.264 single-NAL/FU-A packetization are present.
- Protocol: H.264 depacketization supports single NAL and FU-A runs.
- Transport: UDP send/receive helpers exist.
- Windows: the client receives RTP packets and reports packet statistics only.
- Not implemented: VideoToolbox compression, H.264 decode/render, input forwarding, session negotiation, encryption/authentication.

The README contains older status text and example commands that do not fully match the current capture-probe CLI. Treat the code as authoritative until the documentation is reconciled.

## Work completed on this branch

- Added a zero-copy Annex-B H.264 NAL iterator in `localdesk-protocol`.
- Added tests for mixed 3-byte/4-byte start codes, empty NALs, trailing zero bytes, and invalid input.
- Gated ScreenCaptureKit-only imports with `cfg(target_os = "macos")`, so the macOS crate does not unconditionally import a target-specific dependency on other platforms.

## Next implementation milestones

1. **Validate protocol primitives**
   - Run `cargo fmt --all -- --check`.
   - Run `cargo test --workspace`.
   - Add RTP sequence/timestamp/SSRC consistency checks to FU-A depacketization.
   - Add tests for malformed FU-A sequences and RTP CSRC/extension/padding parsing.

2. **Implement VideoToolbox encoder**
   - Use Apple's VideoToolbox `VTCompressionSession` through a maintained Rust binding or a small Objective-C/Swift FFI shim.
   - Keep the encoder implementation behind `cfg(target_os = "macos")`.
   - Feed the captured `CVPixelBuffer` directly where possible; avoid CPU readback and BGRA conversion.
   - Configure H.264, target bitrate, expected frame rate, keyframe interval, and real-time mode.
   - Extract SPS/PPS from format-description parameter sets and emit them before an IDR access unit.
   - Identify the encoder output format explicitly. Convert AVCC length-prefixed samples to NAL units or Annex-B before RTP packetization; do not guess the format.

3. **Connect capture to encoder safely**
   - Keep ScreenCaptureKit callback work bounded.
   - Do not block the capture callback on UDP or encoding completion.
   - Use a bounded queue and define a drop policy (prefer dropping stale frames over unbounded latency).
   - Preserve presentation timestamps and convert them to a 90 kHz RTP clock.
   - Handle resize/display changes and encoder reconfiguration.

4. **Send RTP over UDP**
   - Packetize each H.264 NAL using the existing RFC 6184 FU-A implementation.
   - Maintain a monotonically wrapping RTP sequence number and stable SSRC per stream.
   - Use one RTP timestamp for all NAL packets belonging to the same access unit.
   - Set the RTP marker bit on the final packet of an access unit, not merely the final fragment of every NAL. The current helper marks the final fragment of each NAL; the sender integration must correct this at access-unit level.

5. **Windows decode and display**
   - Reassemble access units using RTP timestamp, marker, sequence number, and loss detection.
   - Feed H.264 to Media Foundation or another maintained decoder integration.
   - Render decoded frames with a low-latency presentation path.
   - Add explicit receiver buffering and recovery on packet loss/keyframes.

6. **Security before real use**
   - Add authenticated session setup and media encryption (e.g. DTLS-SRTP or an equivalently reviewed design).
   - Do not expose the current unencrypted UDP prototype beyond a trusted test network.

## Important RTP detail

The current `packetize_h264_nal` API works at NAL-unit granularity and sets the marker bit on the last packet of each NAL. RFC 6184 uses the marker bit to indicate the last packet of an access unit. Before sending real encoder output, introduce an access-unit packetizer or allow the caller to set the marker only on the last NAL of each access unit.
