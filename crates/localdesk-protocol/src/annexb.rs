//! H.264 Annex-B byte-stream parsing.
//!
//! Screen encoders commonly expose either AVCC (length-prefixed) or Annex-B
//! (start-code-prefixed) output. RTP packetization consumes one NAL unit at a
//! time, so this module provides a small zero-copy iterator over Annex-B data.

use crate::ProtocolError;

/// Iterate over NAL units in an Annex-B byte stream without copying payloads.
///
/// Both 3-byte and 4-byte start codes are accepted. Leading zero bytes are
/// tolerated. Empty NAL units between adjacent start codes are skipped.
pub fn annex_b_nals(data: &[u8]) -> Result<impl Iterator<Item = &[u8]>, ProtocolError> {
    let starts = find_start_codes(data);
    if starts.is_empty() {
        return Err(ProtocolError::InvalidNal);
    }

    let mut nals = Vec::with_capacity(starts.len());
    for (index, &(prefix, payload_start)) in starts.iter().enumerate() {
        let payload_end = starts
            .get(index + 1)
            .map(|(next_prefix, _)| *next_prefix)
            .unwrap_or(data.len());

        if payload_start > payload_end {
            return Err(ProtocolError::InvalidNal);
        }

        let mut end = payload_end;
        // Annex-B permits trailing_zero_8bits after the final NAL. Strip only
        // zero bytes at the end of each NAL; they belong to the next start
        // code when one exists, but stripping here is harmless for NAL data.
        while end > payload_start && data[end - 1] == 0 {
            end -= 1;
        }

        if end > payload_start {
            nals.push(&data[payload_start..end]);
        }

        let _ = prefix; // documents that both start-code lengths are retained
    }

    if nals.is_empty() {
        return Err(ProtocolError::InvalidNal);
    }

    Ok(nals.into_iter())
}

fn find_start_codes(data: &[u8]) -> Vec<(usize, usize)> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if i + 4 <= data.len() && data[i..i + 4] == [0, 0, 0, 1] {
            starts.push((i, i + 4));
            i += 4;
        } else if data[i..i + 3] == [0, 0, 1] {
            starts.push((i, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    starts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mixed_start_codes_without_copying() {
        let stream = [
            0, 0, 0, 1, 0x67, 0x42, 0, 0, 1, 0x68, 0xce, 0x06,
            0, 0, 0, 1, 0x65, 0xaa, 0xbb,
        ];
        let nals: Vec<&[u8]> = annex_b_nals(&stream).unwrap().collect();
        assert_eq!(nals, vec![&[0x67, 0x42][..], &[0x68, 0xce, 0x06][..], &[0x65, 0xaa, 0xbb][..]]);
    }

    #[test]
    fn skips_empty_units_and_trailing_zero_bytes() {
        let stream = [0, 0, 1, 0x65, 0xaa, 0, 0, 0, 1, 0, 0, 1, 0x41, 0xbb, 0];
        let nals: Vec<&[u8]> = annex_b_nals(&stream).unwrap().collect();
        assert_eq!(nals, vec![&[0x65, 0xaa][..], &[0x41, 0xbb][..]]);
    }

    #[test]
    fn rejects_data_without_start_code() {
        assert!(annex_b_nals(&[0x65, 1, 2]).is_err());
    }
}
