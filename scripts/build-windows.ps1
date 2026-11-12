$ErrorActionPreference = 'Stop'
# Run in a Windows x64 Rust environment with the MSVC toolchain installed.
rustup target add x86_64-pc-windows-msvc
cargo build --release -p localdesk-windows-client --target x86_64-pc-windows-msvc
