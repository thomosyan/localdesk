mod annexb;

pub use annexb::annex_b_nals;

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
        if raw.len() < RTP_HEADER_LEN { return Err(ProtocolError::ShortRtpHeader); }
        let version = raw[0] >> 6;
        if version != RTP_VERSION { return Err(ProtocolError::UnsupportedVersion(version)); }
        let padding = (raw[0] & 0x20) != 0;
        let extension = (raw[0] & 0x10) != 0;
        let csrc_count = (raw[0] & 0x0f) as usize;
        let mut offset = RTP_HEADER_LEN + csrc_count * 4;
        if raw.len() < offset { return Err(ProtocolError::InvalidHeaderLength); }
        if extension {
            if raw.len() < offset + 4 { return Err(ProtocolError::InvalidHeaderLength); }
            let words = u16::from_be_bytes([raw[offset + 2], raw[offset + 3]]) as usize;
            offset += 4 + words * 4;
            if raw.len() < offset { return Err(ProtocolError::InvalidHeaderLength); }
        }
        let mut end = raw.len();
        if padding {
            let pad = *raw.last().ok_or(ProtocolError::InvalidHeaderLength)? as usize;
            if pad == 0 || pad > end.saturating_sub(offset) { return Err(ProtocolError::InvalidHeaderLength); }
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

/// Packetize one Annex-B-independent H.264 NAL unit (without 00 00 01 prefix).
/// Uses RFC 6184 single-NAL packets or FU-A fragmentation.
pub fn packetize_h264_nal(
    nal: &[u8], mtu: usize, first_sequence: u16, timestamp: u32, ssrc: u32,
) -> Result<Vec<RtpPacket>, ProtocolError> {
    if nal.is_empty() { return Err(ProtocolError::InvalidNal); }
    if mtu <= RTP_HEADER_LEN + 2 { return Err(ProtocolError::MtuTooSmall); }
    let max_payload = mtu - RTP_HEADER_LEN;
    if nal.len() <= max_payload {
        return Ok(vec![RtpPacket {
            marker: true, payload_type: H264_PAYLOAD_TYPE, sequence: first_sequence,
            timestamp, ssrc, payload: Bytes::copy_from_slice(nal),
        }]);
    }

    let nal_header = nal[0];
    let indicator = (nal_header & 0xe0) | 28; // FU-A, preserve F and NRI
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
            marker: end,
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
    if packets.is_empty() { return Err(ProtocolError::InvalidNal); }
    if packets.len() == 1 {
        let p = &packets[0].payload;
        if p.is_empty() { return Err(ProtocolError::InvalidNal); }
        if (p[0] & 0x1f) != 28 { return Ok(p.clone()); }
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
        if p.len() < 3 || (p[0] & 0x1f) != 28 { return Err(ProtocolError::UnsupportedH264Packet); }
        let start = (p[1] & 0x80) != 0;
        let end = (p[1] & 0x40) != 0;
        if i == 0 && !start { return Err(ProtocolError::UnsupportedH264Packet); }
        if i > 0 && start { return Err(ProtocolError::UnsupportedH264Packet); }
        if end != (i == packets.len() - 1) { return Err(ProtocolError::UnsupportedH264Packet); }
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
        let packet = RtpPacket { marker: true, payload_type: 96, sequence: 65530,
            timestamp: 123456, ssrc: 42, payload: Bytes::from_static(b"hello") };
        assert_eq!(RtpPacket::decode(&packet.encode()).unwrap(), packet);
    }

    #[test]
    fn h264_single_nal_round_trip() {
        let nal = [0x65, 1, 2, 3, 4];
        let packets = packetize_h264_nal(&nal, 1200, 10, 99, 7).unwrap();
        assert_eq!(depacketize_h264_nal(&packets).unwrap().as_ref(), nal);
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
