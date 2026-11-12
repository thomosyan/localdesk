//! LocalDesk macOS screen capture and H.264 encoding probe.
//!
//! Captured IOSurfaces are queued to a dedicated VideoToolbox worker. This
//! milestone validates local capture, hardware/software H.264 encoding, and
//! optional Annex-B elementary-stream recording; it does not transmit RTP yet.

use anyhow::{Context, Result, bail};
use clap::Parser;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use crate::h264_file::extract_access_unit_nals;
#[cfg(target_os = "macos")]
use apple_cf::iosurface::IOSurface;
#[cfg(target_os = "macos")]
use localdesk_protocol::{RtpPacket, packetize_h264_access_unit};
#[cfg(target_os = "macos")]
use localdesk_transport::{bind_udp, send_rtp};
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
use std::fs::File;
#[cfg(target_os = "macos")]
use std::io::BufWriter;
#[cfg(target_os = "macos")]
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::sync::mpsc::{self, SyncSender, TrySendError};
#[cfg(target_os = "macos")]
use std::thread;
#[cfg(target_os = "macos")]
use tracing::{error, info, warn};

#[cfg(target_os = "macos")]
use crate::encoder::{H264Encoder, H264EncoderConfig};
#[cfg(target_os = "macos")]
use crate::h264_file::H264AnnexBWriter;

#[cfg(target_os = "macos")]
#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    /// Returns a borrowed IOSurfaceRef associated with the pixel buffer.
    fn CVPixelBufferGetIOSurface(pixel_buffer: *mut c_void) -> *mut c_void;
}

#[cfg(target_os = "macos")]
pub mod encoder;
#[cfg(target_os = "macos")]
mod h264_file;

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

    /// Optional path for an Annex-B H.264 elementary stream (for example capture.h264)
    #[arg(long)]
    output: Option<PathBuf>,

    /// Optional RTP/UDP destination, for example 10.5.1.85:5004
    #[arg(long)]
    target: Option<String>,

    /// Maximum RTP packet size in bytes
    #[arg(long, default_value_t = 1200)]
    mtu: usize,
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
    if !(64..=65_507).contains(&args.mtu) {
        bail!("--mtu must be between 64 and 65507 bytes");
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
    let output_path = args.output.clone();
    let rtp_mtu = args.mtu;
    let rtp_fps = args.fps;

    // The UDP thread owns one Tokio runtime and one socket for its lifetime.
    // The encoder only performs non-blocking sends into this bounded queue.
    let (rtp_tx, udp_worker) = if let Some(target) = args.target.clone() {
        let (tx, rx) = mpsc::sync_channel::<Vec<RtpPacket>>(8);
        let (udp_ready_tx, udp_ready_rx) = mpsc::sync_channel::<std::result::Result<(), String>>(1);
        let udp_stats = Arc::clone(&stats);
        let udp_target = target.clone();

        let udp_worker = thread::Builder::new()
            .name("localdesk-udp-sender".into())
            .spawn(move || -> Result<()> {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .context("Could not create UDP Tokio runtime")?;

                let socket = match runtime.block_on(bind_udp("0.0.0.0:0")) {
                    Ok(socket) => {
                        let _ = udp_ready_tx.send(Ok(()));
                        socket
                    }
                    Err(error) => {
                        let message = format!("{error:#}");
                        let _ = udp_ready_tx.send(Err(message.clone()));
                        bail!("Could not initialize UDP socket: {message}");
                    }
                };

                while let Ok(packets) = rx.recv() {
                    for packet in packets {
                        match runtime.block_on(send_rtp(&socket, &udp_target, &packet)) {
                            Ok(bytes) => {
                                udp_stats.rtp_packets.fetch_add(1, Ordering::Relaxed);
                                udp_stats
                                    .rtp_bytes
                                    .fetch_add(bytes as u64, Ordering::Relaxed);
                            }
                            Err(error) => {
                                udp_stats.rtp_send_errors.fetch_add(1, Ordering::Relaxed);
                                error!(%error, "RTP UDP send failed");
                            }
                        }
                    }
                }
                Ok(())
            })
            .context("Failed to spawn UDP sender worker")?;

        udp_ready_rx
            .recv()
            .context("UDP worker exited before socket initialization")?
            .map_err(|message| anyhow::anyhow!("UDP initialization failed: {message}"))?;

        info!(destination = %target, mtu = args.mtu, queue_capacity = 8,
              "RTP/UDP transmission enabled");
        (Some(tx), Some(udp_worker))
    } else {
        info!("RTP/UDP transmission disabled; use --target to enable it");
        (None, None)
    };

    let encoder_rtp_tx = rtp_tx.clone();

    let encoder_worker = thread::Builder::new()
        .name("localdesk-h264-encoder".into())
        .spawn(move || -> Result<()> {
            let mut rtp_sequence: u16 = 0;
            const RTP_SSRC: u32 = 0x4c_44_53_4b;
            let mut output = match output_path {
                Some(path) => {
                    let file = File::create(&path).with_context(|| {
                        format!("Could not create H.264 output file {}", path.display())
                    })?;
                    info!(path = %path.display(), "Recording Annex-B H.264 elementary stream");
                    Some(H264AnnexBWriter::new(BufWriter::new(file)))
                }
                None => None,
            };

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
                                worker_stats
                                    .encoded_bytes
                                    .fetch_add(frame.data.len() as u64, Ordering::Relaxed);

                                if let Some(writer) = output.as_mut() {
                                    let bytes = writer.write_frame(&frame).with_context(|| {
                                        format!("Failed writing encoded frame {frame_index}")
                                    })?;
                                    worker_stats.output_frames.fetch_add(1, Ordering::Relaxed);
                                    worker_stats
                                        .output_bytes
                                        .fetch_add(bytes as u64, Ordering::Relaxed);
                                }

                                if let Some(sender) = encoder_rtp_tx.as_ref() {
                                    match extract_access_unit_nals(&frame) {
                                        Ok(nals) => {
                                            let nal_refs: Vec<&[u8]> =
                                                nals.iter().map(Vec::as_slice).collect();
                                            let timestamp = ((frame_index as u128 * 90_000)
                                                / u128::from(rtp_fps))
                                                as u32;

                                            match packetize_h264_access_unit(
                                                &nal_refs,
                                                rtp_mtu,
                                                rtp_sequence,
                                                timestamp,
                                                RTP_SSRC,
                                            ) {
                                                Ok(packets) => {
                                                    rtp_sequence = rtp_sequence
                                                        .wrapping_add(packets.len() as u16);
                                                    worker_stats
                                                        .rtp_access_units
                                                        .fetch_add(1, Ordering::Relaxed);

                                                    match sender.try_send(packets) {
                                                        Ok(()) => {}
                                                        Err(TrySendError::Full(_)) => {
                                                            worker_stats
                                                                .rtp_queue_drops
                                                                .fetch_add(1, Ordering::Relaxed);
                                                        }
                                                        Err(TrySendError::Disconnected(_)) => {
                                                            worker_stats
                                                                .rtp_queue_drops
                                                                .fetch_add(1, Ordering::Relaxed);
                                                            warn!("UDP sender disconnected");
                                                        }
                                                    }
                                                }
                                                Err(error) => {
                                                    worker_stats
                                                        .rtp_packetize_errors
                                                        .fetch_add(1, Ordering::Relaxed);
                                                    error!(frame_index, %error,
                                                        "RTP packetization failed");
                                                }
                                            }
                                        }
                                        Err(error) => {
                                            worker_stats
                                                .rtp_packetize_errors
                                                .fetch_add(1, Ordering::Relaxed);
                                            error!(frame_index, %error,
                                                "Could not extract H.264 NAL units for RTP");
                                        }
                                    }
                                }
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

            if let Some(writer) = output.as_mut() {
                writer.flush()?;
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
        drop(encoder_tx);
        let worker_result = encoder_worker
            .join()
            .map_err(|_| anyhow::anyhow!("H.264 encoder worker panicked"))?;
        worker_result?;
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
                output_frames = stats.output_frames.load(Ordering::Relaxed),
                output_bytes = stats.output_bytes.load(Ordering::Relaxed),
                rtp_access_units = stats.rtp_access_units.load(Ordering::Relaxed),
                rtp_packets = stats.rtp_packets.load(Ordering::Relaxed),
                rtp_bytes = stats.rtp_bytes.load(Ordering::Relaxed),
                rtp_queue_drops = stats.rtp_queue_drops.load(Ordering::Relaxed),
                rtp_send_errors = stats.rtp_send_errors.load(Ordering::Relaxed),
                rtp_packetize_errors = stats.rtp_packetize_errors.load(Ordering::Relaxed),
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

    let stop_result = encoder_tx.send(EncoderMessage::Stop);
    drop(encoder_tx);
    let worker_result = encoder_worker
        .join()
        .map_err(|_| anyhow::anyhow!("H.264 encoder worker panicked"))?;
    stop_result.context("H.264 encoder worker disconnected before shutdown")?;
    worker_result?;

    // Dropping the final sender closes the queue; the UDP worker drains queued
    // access units before exiting.
    drop(rtp_tx);
    if let Some(worker) = udp_worker {
        worker
            .join()
            .map_err(|_| anyhow::anyhow!("UDP sender worker panicked"))??;
    }

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
        output_frames = stats.output_frames.load(Ordering::Relaxed),
        output_bytes = stats.output_bytes.load(Ordering::Relaxed),
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
    output_frames: AtomicU64,
    output_bytes: AtomicU64,
    rtp_access_units: AtomicU64,
    rtp_packets: AtomicU64,
    rtp_bytes: AtomicU64,
    rtp_queue_drops: AtomicU64,
    rtp_send_errors: AtomicU64,
    rtp_packetize_errors: AtomicU64,
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
        let borrowed_surface = unsafe { CVPixelBufferGetIOSurface(pixel_buffer.as_ptr()) };
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
