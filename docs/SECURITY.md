# Security requirements

The current 0.1 diagnostic UDP path is plaintext and unauthenticated. It is for a trusted isolated lab only and must not carry real desktop frames or input events.

Production requirements before enabling remote control:

1. Pair devices using an out-of-band one-time pairing code or locally verified fingerprint.
2. Authenticate the server; do not silently trust any LAN peer.
3. Protect control traffic with TLS 1.3 and media with DTLS-SRTP (or another reviewed standard).
4. Bind to the LAN interface where practical and apply host firewall allowlists.
5. Require explicit macOS Screen Recording and Accessibility consent.
6. Provide a visible active-session indicator and immediate disconnect control.
7. Never log passwords, pairing secrets, clipboard contents or keystrokes.
8. Do not expose listener ports through router port forwarding.
