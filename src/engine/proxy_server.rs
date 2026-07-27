use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Context, Result};
use rand::seq::IndexedRandom;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tracing::{debug, info};

use super::util;

/// A lightweight HTTP CONNECT forward proxy that rotates its egress source IP.
///
/// On every CONNECT request it binds the outgoing socket to a random IP from the
/// configured pool. This hides the host's real IP and makes each connection look
/// like it originates from a different address (as long as those addresses are
/// routable from the host).
pub struct ProxyServer {
    listen_addr: SocketAddr,
    source_ips: Arc<Vec<IpAddr>>,
}

impl ProxyServer {
    pub fn new(listen_addr: SocketAddr, source_ips: Vec<IpAddr>) -> Self {
        Self {
            listen_addr,
            source_ips: Arc::new(source_ips),
        }
    }

    /// Build a server from a CIDR block (e.g. "10.0.0.0/24").
    pub fn from_cidr(listen_addr: SocketAddr, cidr: &str) -> Result<Self> {
        let ips = generate_ips_from_cidr(cidr)?;
        Ok(Self::new(listen_addr, ips))
    }

    pub async fn run(&self) -> Result<()> {
        if self.source_ips.is_empty() {
            anyhow::bail!("no source IPs configured; proxy cannot rotate egress addresses");
        }
        info!(
            "starting HTTP CONNECT proxy on {} with {} source IPs",
            self.listen_addr,
            self.source_ips.len()
        );

        let listener = TcpListener::bind(self.listen_addr).await?;
        let ips = self.source_ips.clone();

        loop {
            let (client, peer_addr) = listener.accept().await?;
            let ips = ips.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_connection(client, peer_addr, ips).await {
                    debug!("proxy connection from {} error: {}", peer_addr, e);
                }
            });
        }
    }
}

async fn handle_connection(
    mut client: TcpStream,
    peer_addr: SocketAddr,
    source_ips: Arc<Vec<IpAddr>>,
) -> Result<()> {
    // Read the CONNECT request line and headers.
    let mut buf = vec![0u8; 8192];
    let mut header_end = 0usize;
    loop {
        let n = client.read(&mut buf[header_end..]).await?;
        if n == 0 {
            anyhow::bail!("client disconnected before sending CONNECT");
        }
        header_end += n;
        if buf[..header_end].windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if header_end >= buf.len() {
            anyhow::bail!("CONNECT request headers too large");
        }
    }

    let request = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = request.lines();
    let connect_line = lines
        .next()
        .ok_or_else(|| anyhow::anyhow!("empty proxy request"))?;

    let parts: Vec<&str> = connect_line.split_whitespace().collect();
    if parts.len() != 3 || parts[0] != "CONNECT" {
        anyhow::bail!("expected HTTP CONNECT, got: {}", connect_line);
    }

    let target = parts[1];
    let target_addr = SocketAddr::from_str(target)
        .or_else(|_| {
            // If no port was given, default to 443 for CONNECT.
            SocketAddr::from_str(&format!("{}:443", target))
        })
        .with_context(|| format!("invalid CONNECT target: {}", target))?;

    // Pick a random source IP.
    let source_ip = *source_ips
        .choose(&mut rand::rng())
        .ok_or_else(|| anyhow::anyhow!("source IP pool is empty"))?;

    debug!(
        "{} -> {} via source IP {}",
        peer_addr, target_addr, source_ip
    );

    // Bind outbound socket to the chosen source IP.
    let mut outbound = bind_and_connect(target_addr, source_ip).await?;

    // Respond 200 to client.
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;

    // Bidirectional tunnel.
    let (mut client_read, mut client_write) = client.split();
    let (mut outbound_read, mut outbound_write) = outbound.split();

    let upstream = tokio::io::copy(&mut client_read, &mut outbound_write);
    let downstream = tokio::io::copy(&mut outbound_read, &mut client_write);

    let _ = tokio::try_join!(upstream, downstream);
    Ok(())
}

async fn bind_and_connect(target: SocketAddr, source_ip: IpAddr) -> Result<TcpStream> {
    let source_addr = SocketAddr::new(source_ip, 0);

    let socket = if source_ip.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };

    socket
        .bind(source_addr)
        .with_context(|| format!("failed to bind outbound socket to {}", source_addr))?;

    let stream = socket
        .connect(target)
        .await
        .with_context(|| format!("failed to connect to {} from {}", target, source_addr))?;

    Ok(stream)
}

pub fn generate_ips_from_cidr(cidr: &str) -> Result<Vec<IpAddr>> {
    let (addr, prefix) = util::parse_cidr(cidr)?;
    if prefix >= 31 {
        anyhow::bail!("prefix must be <= 30 to have usable hosts");
    }
    let network_u32 = u32::from(addr);
    let mask = !((1u32 << (32 - prefix)) - 1);
    let base = network_u32 & mask;
    let host_count = (1u32 << (32 - prefix)) - 2;

    let mut ips = Vec::with_capacity(host_count as usize);
    for i in 1..=host_count {
        ips.push(IpAddr::V4(Ipv4Addr::from(base + i)));
    }
    Ok(ips)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_ips_from_cidr() {
        let ips = generate_ips_from_cidr("10.0.0.0/30").unwrap();
        assert_eq!(ips.len(), 2);
        assert_eq!(ips[0], IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
        assert_eq!(ips[1], IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)));
    }
}
