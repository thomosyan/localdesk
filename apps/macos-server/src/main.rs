//! LocalDesk macOS screen capture and H.264 encoding probe.
//!
//! Captured IOSurfaces are queued to a dedicated VideoToolbox worker. This
//! milestone validates local capture and hardware/software H.264 encoding;
//! it does not transmit RTP yet.

use anyhow::{Context, Result, bail};
use clap::Parser;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use apple_cf::iosurface::IOSurface;
#[cfg(target_os = "macos")]
use screencapturekit::cm::CMSampleBuffer;
#[cfg(target_os = "macos")]
use screencapturekit::cm::CMSampleBufferExt;
#[cfg(target_os = "macos")]
use screencapturekit::stream::{
    output_trait::SCStreamOutputTrait, output_type::SCStreamOutputType,
};
#[cfg(target_os = "macos")]
use std::ffi::c_void;
#[cfg(target_os = "macos")]
use std::sync::mpsc::{self, SyncSender, TrySendError};
#[cfg(target_os = "macos")]
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::thread;
#[cfg(target_os = "macos")]
use tracing::{error, info, warn};

#[cfg(target_os = "macos")]
use crate::encoder::{H264Encoder, H264EncoderConfig};

#[cfg(target_os = "macos")]
#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    /// Returns a borrowed IOSurfaceRef associated with the pixel buffer.
    fn CVPixelBufferGetIOSurface(pixel_buffer: *mut c_void) -> *mut c_void;
}

#[cfg(target_os = "macos")]
pub mod encoder;

#[derive(Debug, Parser)]
#[command(
    name = "localdesk-server",
    version,
    about = "LocalDesk native macOS screen-capture and H.264 encoding probe"
)]
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
    let display = displays.get(args.display).with_context(|| {
        format!(
            "Display index {} does not exist; {} display(s) found",
            args.display,
            displays.len()
        )
    })?;

    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&[])
        .build()
        .context("Failed to create ScreenCaptureKit display filter")?;

    let frame_interval = CMTime::new(1, args.fps as i32);
    let config = SCStreamConfiguration::new()
        .with_width(args.width)
        .with_height(args.height)
        .with_pixel_format(PixelFormat::YCbCr_420v)
        .with_shows_cursor(true)
        .with_minimum_frame_interval(&frame_interval);

    let stats = Arc::new(CaptureStats::default());
    let (encoder_tx, encoder_rx) = mpsc::sync_channel::<EncoderMessage>(3);
    let (ready_tx, ready_rx) = mpsc::sync_channel::<std::result::Result<(), String>>(1);
    let worker_stats = Arc::clone(&stats);

    let encoder_worker = thread::Builder::new()
        .name("localdesk-h264-encoder".into())
        .spawn(move || -> Result<()> {
            let encoder = match H264Encoder::new(H264EncoderConfig {
                width: args.width,
                height: args.height,
                fps: args.fps,
                ..H264EncoderConfig::default()
            }) {
                Ok(encoder) => {
                    let _ = ready_tx.send(Ok(()));
                    encoder
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    let _ = ready_tx.send(Err(message.clone()));
                    bail!("Failed to initialize H.264 encoder: {message}");
                }
            };

            while let Ok(message) = encoder_rx.recv() {
                match message {
                    EncoderMessage::Frame(surface, frame_index) => {
                        match encoder.encode_frame(&surface, frame_index) {
                            Ok(frame) => {
                                worker_stats.encoded.fetch_add(1, Ordering::Relaxed);
                                worker_stats.encoded_bytes.fetch_add(
                                    frame.data.len() as u64,
                                    Ordering::Relaxed,
                                );
                            }
                            Err(error) => {
                                worker_stats.encode_errors.fetch_add(1, Ordering::Relaxed);
                                error!(frame_index, %error, "H.264 encoding failed");
                            }
                        }
                    }
                    EncoderMessage::Stop => break,
                }
            }
            Ok(())
        })
        .context("Failed to spawn H.264 encoder worker")?;

    ready_rx
        .recv()
        .context("H.264 encoder worker exited before initialization")?
        .map_err(|error| anyhow::anyhow!("H.264 encoder initialization failed: {error}"))?;

    let handler = FrameHandler {
        stats: Arc::clone(&stats),
        encoder_tx: encoder_tx.clone(),
    };

    let mut stream =
        SCStream::new(&filter, &config).context("Failed to create ScreenCaptureKit stream")?;
    stream
        .add_output_handler(handler, SCStreamOutputType::Screen)
        .context("Failed to register ScreenCaptureKit output handler")?;

    info!(
        display_index = args.display,
        requested_width = args.width,
        requested_height = args.height,
        requested_fps = args.fps,
        encoder_queue_capacity = 3,
        "Starting native screen capture and H.264 encoding"
    );
    info!(
        "Grant Screen Recording to the LocalDesk binary (or Terminal during development) if macOS asks."
    );

    if let Err(error) = stream.start_capture() {
        let _ = encoder_tx.send(EncoderMessage::Stop);
        let _ = encoder_worker.join();
        return Err(error).context("ScreenCaptureKit failed to start capture");
    }

    let started = Instant::now();
    let mut last_report = Instant::now();
    let mut last_frames = 0_u64;
    loop {
        thread::sleep(Duration::from_millis(250));
        let elapsed = started.elapsed();
        if last_report.elapsed() >= Duration::from_secs(1) {
            let total = stats.frames.load(Ordering::Relaxed);
            let fps =
                total.saturating_sub(last_frames) as f64 / last_report.elapsed().as_secs_f64();
            info!(
                captured = total,
                queued = stats.queued.load(Ordering::Relaxed),
                encoded = stats.encoded.load(Ordering::Relaxed),
                dropped = stats.dropped.load(Ordering::Relaxed),
                encode_errors = stats.encode_errors.load(Ordering::Relaxed),
                encoded_bytes = stats.encoded_bytes.load(Ordering::Relaxed),
                measured_fps = format_args!("{fps:.1}"),
                width = stats.width.load(Ordering::Relaxed),
                height = stats.height.load(Ordering::Relaxed),
                elapsed_secs = elapsed.as_secs(),
                "Capture and encoding statistics"
            );
            last_frames = total;
            last_report = Instant::now();
        }
        if args.seconds != 0 && elapsed >= Duration::from_secs(args.seconds) {
            break;
        }
    }

    stream
        .stop_capture()
        .context("Failed to stop ScreenCaptureKit stream")?;

    encoder_tx
        .send(EncoderMessage::Stop)
        .context("H.264 encoder worker disconnected before shutdown")?;
    drop(encoder_tx);
    encoder_worker
        .join()
        .map_err(|_| anyhow::anyhow!("H.264 encoder worker panicked"))??;

    let total = stats.frames.load(Ordering::Relaxed);
    let encoded = stats.encoded.load(Ordering::Relaxed);
    if total == 0 {
        warn!(
            "No frames received. Check System Settings → Privacy & Security → Screen & System Audio Recording, then restart the terminal/app."
        );
        bail!("ScreenCaptureKit produced no frames");
    }
    info!(
        total_frames = total,
        encoded_frames = encoded,
        dropped_frames = stats.dropped.load(Ordering::Relaxed),
        encode_errors = stats.encode_errors.load(Ordering::Relaxed),
        encoded_bytes = stats.encoded_bytes.load(Ordering::Relaxed),
        "Capture and H.264 encoding test completed"
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() -> Result<()> {
    let _ = Args::parse();
    bail!(
        "The LocalDesk screen-capture server can only be built and run on macOS. Build the Windows client separately for Windows."
    )
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct CaptureStats {
    frames: AtomicU64,
    width: AtomicU64,
    height: AtomicU64,
    queued: AtomicU64,
    encoded: AtomicU64,
    dropped: AtomicU64,
    encode_errors: AtomicU64,
    encoded_bytes: AtomicU64,
}

#[cfg(target_os = "macos")]
enum EncoderMessage {
    Frame(IOSurface, u64),
    Stop,
}

#[cfg(target_os = "macos")]
struct FrameHandler {
    stats: Arc<CaptureStats>,
    encoder_tx: SyncSender<EncoderMessage>,
}

#[cfg(target_os = "macos")]
impl SCStreamOutputTrait for FrameHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, output_type: SCStreamOutputType) {
        if output_type != SCStreamOutputType::Screen {
            return;
        }

        let Some(pixel_buffer) = sample.pixel_buffer() else {
            return;
        };

        self.stats
            .width
            .store(pixel_buffer.width() as u64, Ordering::Relaxed);
        self.stats
            .height
            .store(pixel_buffer.height() as u64, Ordering::Relaxed);
        let frame_index = self.stats.frames.fetch_add(1, Ordering::Relaxed);

        // CVPixelBufferGetIOSurface returns a borrowed IOSurfaceRef (+0).
        // Retain it before the CVPixelBuffer wrapper leaves this callback.
        let borrowed_surface =
            unsafe { CVPixelBufferGetIOSurface(pixel_buffer.as_ptr()) };
        let Some(surface) = (unsafe { IOSurface::from_raw_borrowed(borrowed_surface) }) else {
            self.stats.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };

        match self
            .encoder_tx
            .try_send(EncoderMessage::Frame(surface, frame_index))
        {
            Ok(()) => {
                self.stats.queued.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Full(_)) => {
                self.stats.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => {
                self.stats.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
