//! Small, explicit RTP primitives used by LocalDesk.
//! This crate does not provide encryption; production media must use SRTP.

use bytes::{BufMut, Bytes, BytesMut};
use thiserror::Error;

pub const RTP_VERSION: u8 = 2;
pub const RTP_HEADER_LEN: usize = 12;
pub const H264_PAYLOAD_TYPE: u8 = 96;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("RTP packet is shorter than the fixed header")]
    ShortRtpHeader,
    #[error("unsupported RTP version {0}")]
    UnsupportedVersion(u8),
    #[error("invalid RTP header or CSRC length")]
    InvalidHeaderLength,
    #[error("invalid H.264 NAL unit")]
    InvalidNal,
    #[error("invalid or empty H.264 access unit")]
    InvalidAccessUnit,
    #[error("MTU must be larger than RTP header plus FU-A overhead")]
    MtuTooSmall,
    #[error("unsupported H.264 aggregation/fragmentation packet")]
    UnsupportedH264Packet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtpPacket {
    pub marker: bool,
    pub payload_type: u8,
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub payload: Bytes,
}

impl RtpPacket {
    pub fn encode(&self) -> Bytes {
        let mut out = BytesMut::with_capacity(RTP_HEADER_LEN + self.payload.len());
        out.put_u8((RTP_VERSION << 6) | 0); // V=2, P=0, X=0, CC=0
        out.put_u8(((self.marker as u8) << 7) | (self.payload_type & 0x7f));
        out.put_u16(self.sequence);
        out.put_u32(self.timestamp);
        out.put_u32(self.ssrc);
        out.extend_from_slice(&self.payload);
        out.freeze()
    }

    pub fn decode(raw: &[u8]) -> Result<Self, ProtocolError> {
        if raw.len() < RTP_HEADER_LEN {
            return Err(ProtocolError::ShortRtpHeader);
        }
        let version = raw[0] >> 6;
        if version != RTP_VERSION {
            return Err(ProtocolError::UnsupportedVersion(version));
        }
        let padding = (raw[0] & 0x20) != 0;
        let extension = (raw[0] & 0x10) != 0;
        let csrc_count = (raw[0] & 0x0f) as usize;
        let mut offset = RTP_HEADER_LEN + csrc_count * 4;
        if raw.len() < offset {
            return Err(ProtocolError::InvalidHeaderLength);
        }
        if extension {
            if raw.len() < offset + 4 {
                return Err(ProtocolError::InvalidHeaderLength);
            }
            let words = u16::from_be_bytes([raw[offset + 2], raw[offset + 3]]) as usize;
            offset += 4 + words * 4;
            if raw.len() < offset {
                return Err(ProtocolError::InvalidHeaderLength);
            }
        }
        let mut end = raw.len();
        if padding {
            let pad = *raw.last().ok_or(ProtocolError::InvalidHeaderLength)? as usize;
            if pad == 0 || pad > end.saturating_sub(offset) {
                return Err(ProtocolError::InvalidHeaderLength);
            }
            end -= pad;
        }
        let sequence = u16::from_be_bytes([raw[2], raw[3]]);
        let timestamp = u32::from_be_bytes(raw[4..8].try_into().unwrap());
        let ssrc = u32::from_be_bytes(raw[8..12].try_into().unwrap());
        Ok(Self {
            marker: (raw[1] & 0x80) != 0,
            payload_type: raw[1] & 0x7f,
            sequence,
            timestamp,
            ssrc,
            payload: Bytes::copy_from_slice(&raw[offset..end]),
        })
    }
}

/// Packetize one Annex-B-independent H.264 NAL unit.
/// Uses RFC 6184 single-NAL packets or FU-A fragmentation.
/// The NAL is treated as a complete access unit for marker-bit purposes.
pub fn packetize_h264_nal(
    nal: &[u8],
    mtu: usize,
    first_sequence: u16,
    timestamp: u32,
    ssrc: u32,
) -> Result<Vec<RtpPacket>, ProtocolError> {
    packetize_h264_nal_inner(nal, mtu, first_sequence, timestamp, ssrc, true)
}

/// Packetize all NAL units belonging to one H.264 access unit.
///
/// Sequence numbers are continuous across NAL units. The RTP marker bit is
/// set only on the final packet of the final NAL unit in the access unit.
pub fn packetize_h264_access_unit(
    nals: &[&[u8]],
    mtu: usize,
    first_sequence: u16,
    timestamp: u32,
    ssrc: u32,
) -> Result<Vec<RtpPacket>, ProtocolError> {
    if nals.is_empty() || nals.iter().any(|nal| nal.is_empty()) {
        return Err(ProtocolError::InvalidAccessUnit);
    }

    let mut packets = Vec::new();
    let mut next_sequence = first_sequence;

    for (index, nal) in nals.iter().enumerate() {
        let is_last_nal = index == nals.len() - 1;
        let mut nal_packets =
            packetize_h264_nal_inner(nal, mtu, next_sequence, timestamp, ssrc, is_last_nal)?;

        next_sequence = next_sequence.wrapping_add(nal_packets.len() as u16);
        packets.append(&mut nal_packets);
    }

    Ok(packets)
}

fn packetize_h264_nal_inner(
    nal: &[u8],
    mtu: usize,
    first_sequence: u16,
    timestamp: u32,
    ssrc: u32,
    mark_last_packet: bool,
) -> Result<Vec<RtpPacket>, ProtocolError> {
    if nal.is_empty() {
        return Err(ProtocolError::InvalidNal);
    }
    if mtu <= RTP_HEADER_LEN + 2 {
        return Err(ProtocolError::MtuTooSmall);
    }

    let max_payload = mtu - RTP_HEADER_LEN;
    if nal.len() <= max_payload {
        return Ok(vec![RtpPacket {
            marker: mark_last_packet,
            payload_type: H264_PAYLOAD_TYPE,
            sequence: first_sequence,
            timestamp,
            ssrc,
            payload: Bytes::copy_from_slice(nal),
        }]);
    }

    let nal_header = nal[0];
    let indicator = (nal_header & 0xe0) | 28;
    let nal_type = nal_header & 0x1f;
    let chunk_size = max_payload - 2;
    let chunks: Vec<&[u8]> = nal[1..].chunks(chunk_size).collect();
    let mut packets = Vec::with_capacity(chunks.len());

    for (i, chunk) in chunks.iter().enumerate() {
        let start = i == 0;
        let end = i == chunks.len() - 1;
        let fu_header = ((start as u8) << 7) | ((end as u8) << 6) | nal_type;
        let mut payload = BytesMut::with_capacity(2 + chunk.len());
        payload.put_u8(indicator);
        payload.put_u8(fu_header);
        payload.extend_from_slice(chunk);

        packets.push(RtpPacket {
            marker: end && mark_last_packet,
            payload_type: H264_PAYLOAD_TYPE,
            sequence: first_sequence.wrapping_add(i as u16),
            timestamp,
            ssrc,
            payload: payload.freeze(),
        });
    }

    Ok(packets)
}

/// Reassemble one H.264 NAL from a single-NAL RTP packet or a complete FU-A run.
pub fn depacketize_h264_nal(packets: &[RtpPacket]) -> Result<Bytes, ProtocolError> {
    if packets.is_empty() {
        return Err(ProtocolError::InvalidNal);
    }
    if packets.len() == 1 {
        let p = &packets[0].payload;
        if p.is_empty() {
            return Err(ProtocolError::InvalidNal);
        }
        if (p[0] & 0x1f) != 28 {
            return Ok(p.clone());
        }
    }
    let first = &packets[0].payload;
    if first.len() < 3 || (first[0] & 0x1f) != 28 || (first[1] & 0x80) == 0 {
        return Err(ProtocolError::UnsupportedH264Packet);
    }
    let reconstructed_header = (first[0] & 0xe0) | (first[1] & 0x1f);
    let mut out = BytesMut::new();
    out.put_u8(reconstructed_header);
    for (i, packet) in packets.iter().enumerate() {
        let p = &packet.payload;
        if p.len() < 3 || (p[0] & 0x1f) != 28 {
            return Err(ProtocolError::UnsupportedH264Packet);
        }
        let start = (p[1] & 0x80) != 0;
        let end = (p[1] & 0x40) != 0;
        if i == 0 && !start {
            return Err(ProtocolError::UnsupportedH264Packet);
        }
        if i > 0 && start {
            return Err(ProtocolError::UnsupportedH264Packet);
        }
        if end != (i == packets.len() - 1) {
            return Err(ProtocolError::UnsupportedH264Packet);
        }
        if i > 0 && packet.sequence != packets[i - 1].sequence.wrapping_add(1) {
            return Err(ProtocolError::UnsupportedH264Packet);
        }
        out.extend_from_slice(&p[2..]);
    }
    Ok(out.freeze())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtp_round_trip() {
        let packet = RtpPacket {
            marker: true,
            payload_type: 96,
            sequence: 65530,
            timestamp: 123456,
            ssrc: 42,
            payload: Bytes::from_static(b"hello"),
        };
        assert_eq!(RtpPacket::decode(&packet.encode()).unwrap(), packet);
    }

    #[test]
    fn h264_single_nal_round_trip() {
        let nal = [0x65, 1, 2, 3, 4];
        let packets = packetize_h264_nal(&nal, 1200, 10, 99, 7).unwrap();
        assert_eq!(depacketize_h264_nal(&packets).unwrap().as_ref(), nal);
    }

    #[test]
    fn access_unit_marks_only_final_single_nal_packet() {
        let nals: [&[u8]; 3] = [&[0x67, 1], &[0x68, 2], &[0x65, 3]];
        let packets = packetize_h264_access_unit(&nals, 1200, 10, 99, 7).unwrap();

        assert_eq!(packets.len(), 3);
        assert_eq!(
            packets.iter().map(|p| p.marker).collect::<Vec<_>>(),
            vec![false, false, true]
        );
        assert_eq!(
            packets.iter().map(|p| p.sequence).collect::<Vec<_>>(),
            vec![10, 11, 12]
        );
    }

    #[test]
    fn access_unit_marks_only_last_fragment_of_last_nal() {
        let first_nal = [0x67, 1, 2];
        let mut last_nal = vec![0x65];
        last_nal.extend(0..100);

        let nals: [&[u8]; 2] = [&first_nal, &last_nal];
        let packets = packetize_h264_access_unit(&nals, 32, 100, 99, 7).unwrap();

        assert!(packets.len() > 2);
        assert!(packets.iter().all(|p| p.timestamp == 99));
        assert!(packets.last().unwrap().marker);
        assert!(packets[..packets.len() - 1].iter().all(|p| !p.marker));

        for pair in packets.windows(2) {
            assert_eq!(pair[1].sequence, pair[0].sequence.wrapping_add(1));
        }
    }

    #[test]
    fn access_unit_sequence_numbers_wrap() {
        let nals: [&[u8]; 3] = [&[0x67], &[0x68], &[0x65]];
        let packets = packetize_h264_access_unit(&nals, 1200, 65534, 1, 2).unwrap();

        assert_eq!(
            packets.iter().map(|p| p.sequence).collect::<Vec<_>>(),
            vec![65534, 65535, 0]
        );
        assert!(packets.last().unwrap().marker);
    }

    #[test]
    fn access_unit_rejects_empty_input_and_empty_nals() {
        assert_eq!(
            packetize_h264_access_unit(&[], 1200, 0, 0, 0).unwrap_err(),
            ProtocolError::InvalidAccessUnit
        );
        assert_eq!(
            packetize_h264_access_unit(&[&[0x65], &[]], 1200, 0, 0, 0).unwrap_err(),
            ProtocolError::InvalidAccessUnit
        );
    }

    #[test]
    fn h264_fragmented_nal_round_trip() {
        let mut nal = vec![0x65];
        nal.extend((0..5000).map(|n| (n % 251) as u8));
        let packets = packetize_h264_nal(&nal, 1200, 100, 99, 7).unwrap();
        assert!(packets.len() > 1);
        assert_eq!(depacketize_h264_nal(&packets).unwrap().as_ref(), nal);
        assert!(packets.last().unwrap().marker);
    }
}

mod annexb;

pub use annexb::annex_b_nals;
