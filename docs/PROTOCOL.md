# LocalDesk wire protocol (draft)

## Media

- RTP version 2.
- H.264 dynamic payload type 96.
- RFC 6184 single NAL and FU-A packetization.
- One RTP timestamp per encoded access unit; marker bit on the final packet of an access unit.
- Initial target MTU: 1200 bytes including RTP header.
- Sequence number increments for every RTP packet; timestamp uses a 90 kHz clock.

The protocol crate currently implements RTP fixed-header encode/decode and H.264 NAL/FU-A packetization helpers. It does not yet implement a full access-unit assembler, jitter buffer, RTCP feedback, retransmission or SRTP.

## Control messages (planned)

Use length-prefixed JSON during development, then consider a compact binary schema:

- `ClientHello { protocol_version, client_nonce }`
- `ServerHello { protocol_version, server_nonce, capabilities }`
- `Authenticate { proof }`
- `StartSession { display_id, width, height, fps, codec }`
- `Input { sequence, kind, x, y, key_code, modifiers }`
- `RequestKeyframe { reason }`
- `Stats { capture_ms, encode_ms, rtt_ms, decode_ms, render_ms, fps }`

All messages must be authenticated and protected by TLS 1.3 before production use.
