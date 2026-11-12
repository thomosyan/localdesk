//! LocalDesk macOS capture probe.
//!
//! This first native milestone validates ScreenCaptureKit acquisition only.
//! It intentionally does not encode or transmit frames yet.

use anyhow::{bail, Context, Result};
use clap::Parser;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::thread;
#[cfg(target_os = "macos")]
use tracing::{info, warn};
#[cfg(target_os = "macos")]
use screencapturekit::stream::{
    output_trait::SCStreamOutputTrait,
    output_type::SCStreamOutputType,
};
#[cfg(target_os = "macos")]
use screencapturekit::cm::CMSampleBuffer;
#[cfg(target_os = "macos")]
use screencapturekit::cm::CMSampleBufferExt;
#[derive(Debug, Parser)]
#[command(name = "localdesk-server", version, about = "LocalDesk native macOS screen-capture probe")]
struct Args {
    /// Display index (0 = first display returned by ScreenCaptureKit)
    #[arg(long, default_value_t = 0)]
    display: usize,

    /// Requested capture width
    #[arg(long, default_value_t = 1920)]
    width: u32,

    /// Requested capture height
    #[arg(long, default_value_t = 1080)]
    height: u32,

    /// Requested maximum capture rate
    #[arg(long, default_value_t = 60)]
    fps: u32,

    /// Capture duration in seconds; 0 means run until Ctrl+C
    #[arg(long, default_value_t = 15)]
    seconds: u64,
}

#[cfg(target_os = "macos")]
fn main() -> Result<()> {
    use screencapturekit::prelude::*;

    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    let args = Args::parse();
    if args.width == 0 || args.height == 0 || args.fps == 0 || args.fps > 120 {
        bail!("width/height/fps must be positive; fps must be <= 120");
    }

    let content = SCShareableContent::get()
        .context("ScreenCaptureKit could not enumerate displays. Check Screen Recording permission and restart the app.")?;
    let displays = content.displays();
    let display = displays
        .get(args.display)
        .with_context(|| format!("Display index {} does not exist; {} display(s) found", args.display, displays.len()))?;

    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&[])
        .build()
        .context("Failed to create ScreenCaptureKit display filter")?;

    let frame_interval = CMTime::new(1, args.fps as i32);
    let config = SCStreamConfiguration::new()
        .with_width(args.width)
        .with_height(args.height)
        // 4:2:0 is a better starting point for the later H.264/HEVC encoder
        // than BGRA, and avoids an unnecessary full-frame conversion.
        .with_pixel_format(PixelFormat::YCbCr_420v)
        .with_shows_cursor(true)
        .with_minimum_frame_interval(&frame_interval);

    let stats = Arc::new(CaptureStats::default());
    let handler = FrameHandler { stats: Arc::clone(&stats) };

    let mut stream = SCStream::new(&filter, &config)
        .context("Failed to create ScreenCaptureKit stream")?;
    stream
        .add_output_handler(handler, SCStreamOutputType::Screen)
        .context("Failed to register ScreenCaptureKit output handler")?;

    info!(
        display_index = args.display,
        requested_width = args.width,
        requested_height = args.height,
        requested_fps = args.fps,
        "Starting native screen capture"
    );
    info!("Grant Screen Recording to the LocalDesk binary (or Terminal during development) if macOS asks.");

    stream.start_capture().context("ScreenCaptureKit failed to start capture")?;

    let started = Instant::now();
    let mut last_report = Instant::now();
    let mut last_frames = 0_u64;
    loop {
        thread::sleep(Duration::from_millis(250));
        let elapsed = started.elapsed();
        if last_report.elapsed() >= Duration::from_secs(1) {
            let total = stats.frames.load(Ordering::Relaxed);
            let fps = total.saturating_sub(last_frames) as f64 / last_report.elapsed().as_secs_f64();
            let width = stats.width.load(Ordering::Relaxed);
            let height = stats.height.load(Ordering::Relaxed);
            info!(frames = total, measured_fps = format_args!("{fps:.1}"), width, height, elapsed_secs = elapsed.as_secs(), "Capture statistics");
            last_frames = total;
            last_report = Instant::now();
        }
        if args.seconds != 0 && elapsed >= Duration::from_secs(args.seconds) {
            break;
        }
    }

    stream.stop_capture().context("Failed to stop ScreenCaptureKit stream")?;
    let total = stats.frames.load(Ordering::Relaxed);
    if total == 0 {
        warn!("No frames received. Check System Settings → Privacy & Security → Screen & System Audio Recording, then restart the terminal/app.");
        bail!("ScreenCaptureKit produced no frames");
    }
    info!(total_frames = total, "Capture test completed successfully");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() -> Result<()> {
    let _ = Args::parse();
    bail!("The LocalDesk screen-capture server can only be built and run on macOS. Build the Windows client separately for Windows.")
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct CaptureStats {
    frames: AtomicU64,
    width: AtomicU64,
    height: AtomicU64,
}

#[cfg(target_os = "macos")]
struct FrameHandler {
    stats: Arc<CaptureStats>,
}

#[cfg(target_os = "macos")]
impl SCStreamOutputTrait for FrameHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, output_type: SCStreamOutputType) {
        match output_type {
            SCStreamOutputType::Screen => {}
            _ => return,
        }
        if let Some(pixel_buffer) = sample.pixel_buffer() {
            self.stats.width.store(pixel_buffer.width() as u64, Ordering::Relaxed);
            self.stats.height.store(pixel_buffer.height() as u64, Ordering::Relaxed);
            self.stats.frames.fetch_add(1, Ordering::Relaxed);
        }
    }
}
