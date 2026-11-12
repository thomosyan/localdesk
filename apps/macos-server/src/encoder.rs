//! Hardware H.264 encoding through Apple's VideoToolbox framework.
//!
//! ScreenCaptureKit frames are IOSurface-backed. This wrapper accepts that
//! IOSurface directly so callers do not need to read pixels back to CPU memory.

use apple_cf::cm::CMTime;
use apple_cf::iosurface::IOSurface;
use anyhow::Result;
use videotoolbox::prelude::{Codec, CompressionSession, EncodedFrame};

/// Configuration for a low-latency H.264 compression session.
#[derive(Debug, Clone, Copy)]
pub struct H264EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub keyframe_interval: u32,
}

impl Default for H264EncoderConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate: 8_000_000,
            keyframe_interval: 120,
        }
    }
}

/// Synchronous VideoToolbox H.264 encoder.
///
/// Call this from a dedicated worker thread, not from the ScreenCaptureKit
/// output callback: `encode` waits for the encoded frame to be produced.
pub struct H264Encoder {
    session: CompressionSession,
    fps: u32,
}

impl H264Encoder {
    pub fn new(config: H264EncoderConfig) -> Result<Self> {
        anyhow::ensure!(
            config.width > 0
                && config.height > 0
                && config.fps > 0
                && config.bitrate > 0
                && config.keyframe_interval > 0,
            "encoder dimensions, fps, bitrate and keyframe interval must be positive"
        );

        let session = CompressionSession::builder(
            config.width as i32,
            config.height as i32,
            Codec::H264,
        )
        .with_real_time(true)
        .with_average_bit_rate(config.bitrate)
        .with_expected_frame_rate(config.fps as f64)
        .with_max_keyframe_interval(config.keyframe_interval)
        .build()?;

        Ok(Self {
            session,
            fps: config.fps,
        })
    }

    /// Encode one IOSurface-backed frame.
    ///
    /// The timestamp is expressed in frame ticks at the configured frame rate.
    /// The returned `EncodedFrame::data` is AVCC (length-prefixed NAL units),
    /// not Annex-B; SPS/PPS are carried by the sample buffer's format description.
    pub fn encode_frame(&self, surface: &IOSurface, frame_index: u64) -> Result<EncodedFrame> {
        let pts = CMTime::new(frame_index as i64, self.fps as i32);
        Ok(self.session.encode(surface, pts)?)
    }
}
