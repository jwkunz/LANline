//! AIVDM output: re-armor decoded frames into NMEA 0183 `!AIVDM` sentences and
//! fan them out over TCP (OpenCPN / AIS-catcher / aisdispatcher ingest this).

use super::message::{armor, nmea_checksum};
use bytes::Bytes;
use std::net::{IpAddr, SocketAddr};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

/// Build one (single-fragment) `!AIVDM` sentence from a big-endian payload.
/// AIS frames up to 3 slots still fit one sentence for our consumers.
pub fn aivdm(payload: &[u8], bit_len: usize, channel: char) -> String {
    let bits: Vec<bool> = (0..bit_len).map(|i| (payload[i / 8] >> (7 - i % 8)) & 1 == 1).collect();
    let (armored, fill) = armor(&bits);
    let body = format!("AIVDM,1,1,,{channel},{armored},{fill}");
    let cs = nmea_checksum(&body);
    format!("!{body}*{cs:02X}\r\n")
}

/// Bind the AIVDM TCP feed and fan `rx` out to every client. `port == 0`
/// disables it. Returns the bound port (0 when disabled).
pub fn serve(bind: IpAddr, port: u16, tx: broadcast::Sender<Bytes>) -> u16 {
    if port == 0 {
        return 0;
    }
    let listener = match std::net::TcpListener::bind(SocketAddr::new(bind, port)) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("ais: cannot bind tcp/{port}: {e}; AIVDM feed disabled");
            return 0;
        }
    };
    let bound = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    if listener.set_nonblocking(true).is_err() {
        return 0;
    }

    tokio::spawn(async move {
        let listener = match TcpListener::from_std(listener) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("ais: listener adopt failed: {e}");
                return;
            }
        };
        tracing::info!("ais: tcp/{bound} ready (AIVDM feed)");
        loop {
            let Ok((mut sock, peer)) = listener.accept().await else {
                continue;
            };
            let _ = sock.set_nodelay(true);
            let mut rx = tx.subscribe();
            tokio::spawn(async move {
                tracing::debug!("ais: client {peer} connected");
                loop {
                    match rx.recv().await {
                        Ok(line) => {
                            if sock.write_all(&line).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                tracing::debug!("ais: client {peer} gone");
            });
        }
    });

    bound
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ais::message::unarmor;

    #[test]
    fn aivdm_round_trips_a_known_payload() {
        let (bytes, len) = unarmor("177KQJ5000G?tO`K>RA1wUbN0TKH", 0).unwrap();
        let s = aivdm(&bytes, len, 'A');
        assert!(s.starts_with("!AIVDM,1,1,,A,177KQJ5000G?tO`K>RA1wUbN0TKH,0*"));
        assert!(s.ends_with("\r\n"));
        // checksum byte is valid hex
        let star = s.rfind('*').unwrap();
        assert_eq!(s[star + 1..star + 3].chars().filter(|c| c.is_ascii_hexdigit()).count(), 2);
    }
}
