//! UDP transport helpers. No encryption is provided in this prototype.
use anyhow::Result;
use localdesk_protocol::RtpPacket;
use tokio::net::UdpSocket;

pub async fn bind_udp(address: &str) -> Result<UdpSocket> {
    Ok(UdpSocket::bind(address).await?)
}

pub async fn send_rtp(socket: &UdpSocket, target: &str, packet: &RtpPacket) -> Result<usize> {
    Ok(socket.send_to(&packet.encode(), target).await?)
}

pub async fn receive_rtp(
    socket: &UdpSocket,
    buffer: &mut [u8],
) -> Result<(RtpPacket, std::net::SocketAddr)> {
    let (size, peer) = socket.recv_from(buffer).await?;
    let packet = RtpPacket::decode(&buffer[..size])?;
    Ok((packet, peer))
}
