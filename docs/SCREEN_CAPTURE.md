# Native macOS screen capture

LocalDesk 0.2 introduces a native ScreenCaptureKit capture probe using the
`screencapturekit` Rust bindings. It captures the selected display at a
requested size/rate and reports frame count, measured FPS, and actual pixel
buffer dimensions. It does not yet encode or transmit frames.

## Requirements

- Apple Silicon Mac (native `aarch64-apple-darwin` build)
- macOS 12.3 or newer; build with macOS 13 SDK or later
- Xcode Command Line Tools / macOS SDK
- Screen Recording permission for the executable (during development, usually
  grant it to Terminal or the IDE, then restart that app)

## Run

```bash
xcode-select --install
rustup target add aarch64-apple-darwin
cargo run -p localdesk-macos-server --target aarch64-apple-darwin -- \
  --display 0 --width 1920 --height 1080 --fps 60 --seconds 15
```

Use `--seconds 0` to keep capturing until Ctrl+C. The first launch may trigger
macOS Screen Recording permission. Grant it under System Settings → Privacy &
Security → Screen & System Audio Recording, then restart Terminal and run again.

## Pixel format

The stream requests `YCbCr_420v`, a 4:2:0 format suitable for the next H.264 / HEVC
encoding stage. The callback currently only reads the pixel-buffer dimensions;
it deliberately does not lock/copy the pixels to CPU memory.

## Cross-compilation policy

- macOS server: build natively on macOS using the Apple SDK. ScreenCaptureKit
  cannot be meaningfully cross-built on Windows/Linux without Apple's SDK.
- Windows client: from Apple Silicon macOS, use the GNU target and MinGW-w64:
  `brew install mingw-w64`, then `./scripts/build-windows.sh`.
- The MSVC Rust target is not an officially supported cross-build target from
  non-Windows hosts. Use a Windows build runner for an MSVC release build.

The Windows cross-build script will become fully usable as the client gains
native Windows dependencies. At this milestone it is a workspace build path,
not a signed production installer.
