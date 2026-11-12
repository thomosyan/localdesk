//! AVCC-to-Annex-B writer for encoded H.264 access units.

use anyhow::{Context, Result, bail};
use std::io::Write;
use videotoolbox::prelude::EncodedFrame;

const START_CODE: &[u8; 4] = &[0, 0, 0, 1];

/// Writes VideoToolbox AVCC frames as an Annex-B H.264 elementary stream.
pub struct H264AnnexBWriter<W> {
    output: W,
}

impl<W: Write> H264AnnexBWriter<W> {
    pub fn new(output: W) -> Self {
        Self { output }
    }

    /// Writes one complete encoded access unit and returns the number of bytes written.
    pub fn write_frame(&mut self, frame: &EncodedFrame) -> Result<usize> {
        let sample_buffer = frame
            .cm_sample_buffer()
            .context("VideoToolbox frame has no CMSampleBuffer")?;
        let format_description = sample_buffer
            .format_description()
            .context("Encoded sample has no format description")?;
        let parameter_sets = format_description
            .video_parameter_sets()
            .map_err(|status| anyhow::anyhow!("Could not read H.264 parameter sets (OSStatus {status})"))?;

        let length_size = usize::try_from(parameter_sets.nal_unit_header_length)
            .context("Negative AVCC NAL length size")?;
        if !matches!(length_size, 1 | 2 | 4) {
            bail!("Unsupported AVCC NAL length size: {length_size}; expected 1, 2, or 4");
        }

        let nals = parse_avcc_nals(&frame.data, length_size)?;
        let mut written = 0usize;

        // Parameter sets are required to initialize a decoder after joining the
        // stream or seeking. Inject them before each sync access unit unless
        // VideoToolbox already included the corresponding NAL in the access unit.
        if sample_buffer.is_sync_sample() {
            let has_sps = nals.iter().any(|nal| nal_type(nal) == Some(7));
            let has_pps = nals.iter().any(|nal| nal_type(nal) == Some(8));
            let mut injected_sps = has_sps;
            let mut injected_pps = has_pps;

            for parameter_set in &parameter_sets.parameter_sets {
                match nal_type(parameter_set) {
                    Some(7) if !injected_sps => {
                        self.output.write_all(START_CODE)?;
                        self.output.write_all(parameter_set)?;
                        written += START_CODE.len() + parameter_set.len();
                        injected_sps = true;
                    }
                    Some(8) if !injected_pps => {
                        self.output.write_all(START_CODE)?;
                        self.output.write_all(parameter_set)?;
                        written += START_CODE.len() + parameter_set.len();
                        injected_pps = true;
                    }
                    _ => {}
                }
            }

            if !injected_sps || !injected_pps {
                bail!(
                    "Sync sample is missing SPS/PPS (SPS present: {injected_sps}, PPS present: {injected_pps})"
                );
            }
        }

        for nal in nals {
            self.output.write_all(START_CODE)?;
            self.output.write_all(nal)?;
            written += START_CODE.len() + nal.len();
        }

        Ok(written)
    }

    pub fn flush(&mut self) -> Result<()> {
        self.output.flush().context("Failed to flush H.264 output")
    }

    pub fn into_inner(self) -> W {
        self.output
    }
}

fn nal_type(nal: &[u8]) -> Option<u8> {
    nal.first().map(|header| header & 0x1f)
}

/// Parses AVCC length-prefixed NAL units without copying their payloads.
fn parse_avcc_nals(data: &[u8], length_size: usize) -> Result<Vec<&[u8]>> {
    if !matches!(length_size, 1 | 2 | 4) {
        bail!("Invalid AVCC NAL length size: {length_size}");
    }

    let mut nals = Vec::new();
    let mut offset = 0usize;

    while offset < data.len() {
        let prefix_end = offset
            .checked_add(length_size)
            .context("AVCC length prefix offset overflow")?;
        let prefix = data
            .get(offset..prefix_end)
            .context("Truncated AVCC NAL length prefix")?;

        let mut nal_len = 0usize;
        for byte in prefix {
            nal_len = nal_len
                .checked_mul(256)
                .and_then(|value| value.checked_add(usize::from(*byte)))
                .context("AVCC NAL length overflow")?;
        }
        if nal_len == 0 {
            bail!("AVCC access unit contains an empty NAL unit");
        }

        let nal_start = prefix_end;
        let nal_end = nal_start
            .checked_add(nal_len)
            .context("AVCC NAL end offset overflow")?;
        let nal = data
            .get(nal_start..nal_end)
            .context("AVCC NAL length exceeds remaining access-unit data")?;
        if nal_type(nal).is_none() {
            bail!("AVCC NAL unit has no NAL header");
        }

        nals.push(nal);
        offset = nal_end;
    }

    if nals.is_empty() {
        bail!("VideoToolbox returned an empty H.264 access unit");
    }
    Ok(nals)
}

#[cfg(test)]
mod tests {
    use super::parse_avcc_nals;

    #[test]
    fn parses_four_byte_avcc_lengths() {
        let data = [0, 0, 0, 2, 0x65, 0xaa, 0, 0, 0, 1, 0x41];
        let nals = parse_avcc_nals(&data, 4).unwrap();
        assert_eq!(nals, vec![&[0x65, 0xaa][..], &[0x41][..]]);
    }

    #[test]
    fn parses_one_and_two_byte_lengths() {
        assert_eq!(
            parse_avcc_nals(&[2, 0x67, 0x01], 1).unwrap(),
            vec![&[0x67, 0x01][..]]
        );
        assert_eq!(
            parse_avcc_nals(&[0, 2, 0x68, 0x02], 2).unwrap(),
            vec![&[0x68, 0x02][..]]
        );
    }

    #[test]
    fn rejects_truncated_and_zero_length_nals() {
        assert!(parse_avcc_nals(&[0, 0], 4).is_err());
        assert!(parse_avcc_nals(&[0, 0, 0, 0], 4).is_err());
        assert!(parse_avcc_nals(&[0, 0, 0, 3, 0x65], 4).is_err());
    }
}
