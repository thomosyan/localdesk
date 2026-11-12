use anyhow::Result;
use clap::Parser;
use localdesk_transport::bind_udp;
use std::time::Instant;
use tracing::{info, warn};

#[derive(Debug, Parser)]
#[command(name = "localdesk-client", about = "LocalDesk Windows receiver prototype")]
struct Args {
    /// UDP address to listen on, e.g. 0.0.0.0:5004
    #[arg(long, default_value = "0.0.0.0:5004")]
    listen: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let args = Args::parse();
    let socket = bind_udp(&args.listen).await?;
    info!(listen = %args.listen, "LocalDesk RTP diagnostic receiver listening");
    warn!("This build only counts RTP packets. Video decode, rendering and input are not implemented.");

    let mut buffer = vec![0u8; 65_535];
    let mut total: u64 = 0;
    let mut last_report = Instant::now();
    let mut last_seq: Option<u16> = None;
    let mut lost_estimate: u64 = 0;

    loop {
        let (packet, peer) = localdesk_transport::receive_rtp(&socket, &mut buffer).await?;
        total += 1;
        if let Some(previous) = last_seq {
            let expected = previous.wrapping_add(1);
            let delta = packet.sequence.wrapping_sub(expected);
            if delta < 0x8000 { lost_estimate += delta as u64; }
        }
        last_seq = Some(packet.sequence);
        if last_report.elapsed().as_secs_f64() >= 1.0 {
            info!(peer = %peer, packets = total, estimated_missing = lost_estimate,
                last_payload_type = packet.payload_type, "RTP receive statistics");
            last_report = Instant::now();
        }
    }
}
