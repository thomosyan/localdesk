# LocalDesk architecture (0.1)

## Processes

- `localdesk-macos-server`: macOS host. Owns screen capture, encoder, session and input injection.
- `localdesk-windows-client`: Windows viewer. Owns discovery, session UI, jitter buffer, decoder, renderer and input capture.
- `localdesk-protocol`: platform-neutral packet and message definitions.
- `localdesk-transport`: UDP socket primitives.

## Media path

ScreenCaptureKit -> pixel buffer -> VideoToolbox H.264 encoder -> RFC 6184 RTP packetizer -> UDP -> bounded jitter/reorder buffer -> H.264 depacketizer -> hardware decoder -> D3D11 renderer.

Start with H.264 for interoperability. Add HEVC only after measuring hardware encode/decode support on the actual Mac and Windows GPU.

## Control path

Use a separate reliable, authenticated TLS 1.3 connection for session setup, keyboard/mouse events, resize requests, clipboard metadata and health messages. Media should use SRTP (DTLS-SRTP key establishment) or another reviewed authenticated-encryption construction. Do not send input events or media in plaintext in production.

## Latency policy

- 60 FPS target means a frame budget of 16.67 ms.
- Keep capture and encode queues bounded (ideally 1–2 frames); drop stale frames instead of building latency.
- UDP media packets should stay below path MTU; start with RTP packet size around 1200 bytes to avoid IP fragmentation.
- Use sequence numbers, timestamps, jitter/reorder handling and keyframe requests.
- Measure capture, encode, network, decode and render separately.

## Discovery

For the first LAN release, use explicit IP entry. Add mDNS later. Avoid broadcast-only discovery as the sole connection mechanism because VLANs and Wi-Fi client isolation can block it.
