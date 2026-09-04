//! Beast binary output: the de-facto interchange format for 1090 MHz feeders
//! (readsb / tar1090 / Virtual Radar Server). Each frame is
//! `0x1a <type> <6-byte 12 MHz timestamp> <1-byte signal> <mode-s frame>`,
//! with every `0x1a` in the payload doubled.

use bytes::{BufMut, Bytes, BytesMut};
use std::net::{IpAddr, SocketAddr};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

/// Encode one Mode S frame (7 or 14 bytes) as a Beast message.
pub fn encode(frame: &[u8], rssi_dbfs: f32, clock_12mhz: u64) -> Bytes {
    let type_byte = match frame.len() {
        2 => 0x31,
        7 => 0x32,
        14 => 0x33,
        _ => return Bytes::new(),
    };

    let mut out = BytesMut::with_capacity(2 + 14 + frame.len() * 2 + 4);
    out.put_u8(0x1a);
    out.put_u8(type_byte);

    let ts = (clock_12mhz & 0x0000_FFFF_FFFF_FFFF).to_be_bytes();
    push_escaped(&mut out, &ts[2..8]); // low 6 bytes

    // Map RSSI (dBFS, ~[-60, 0]) to an 8-bit linear-ish signal level.
    let level = (10f32.powf(rssi_dbfs / 20.0) * 255.0).clamp(1.0, 255.0) as u8;
    push_escaped(&mut out, &[level]);

    push_escaped(&mut out, frame);
    out.freeze()
}

fn push_escaped(out: &mut BytesMut, bytes: &[u8]) {
    for &b in bytes {
        out.put_u8(b);
        if b == 0x1a {
            out.put_u8(0x1a);
        }
    }
}

/// Bind the Beast TCP listener and fan `rx` out to every connected client.
/// `port == 0` disables the feed. Returns the bound port (0 when disabled).
pub fn serve(bind: IpAddr, port: u16, tx: broadcast::Sender<Bytes>) -> u16 {
    if port == 0 {
        return 0;
    }
    let listener = match std::net::TcpListener::bind(SocketAddr::new(bind, port)) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("beast: cannot bind tcp/{port}: {e}; feed disabled");
            return 0;
        }
    };
    let bound = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    if let Err(e) = listener.set_nonblocking(true) {
        tracing::warn!("beast: set_nonblocking: {e}; feed disabled");
        return 0;
    }

    tokio::spawn(async move {
        let listener = match TcpListener::from_std(listener) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("beast: listener adopt failed: {e}");
                return;
            }
        };
        tracing::info!("beast: tcp/{bound} ready (Mode S binary feed)");
        loop {
            let Ok((mut sock, peer)) = listener.accept().await else {
                continue;
            };
            let _ = sock.set_nodelay(true);
            let mut rx = tx.subscribe();
            tokio::spawn(async move {
                tracing::debug!("beast: client {peer} connected");
                loop {
                    match rx.recv().await {
                        Ok(frame) => {
                            if sock.write_all(&frame).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                tracing::debug!("beast: client {peer} gone");
            });
        }
    });

    bound
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_have_delimiter_type_and_escaping() {
        let f = encode(&[0x8d, 0x1a, 0x00], -10.0, 0x0102_0304_0506);
        // 0x1a 0x32? -> len 3 isn't a valid mode-s length, expect empty
        assert!(f.is_empty());

        let frame = [0u8; 14];
        let b = encode(&frame, -10.0, 0x00_00_00_00_00_01);
        assert_eq!(b[0], 0x1a);
        assert_eq!(b[1], 0x33); // long frame
    }

    #[test]
    fn escapes_0x1a_in_payload() {
        let mut frame = [0u8; 7];
        frame[0] = 0x1a;
        let b = encode(&frame, -6.0, 0);
        // delimiter, type, 6 ts bytes, 1 signal, then payload with the first
        // byte (0x1a) doubled.
        let payload = &b[2 + 6 + 1..];
        assert_eq!(payload[0], 0x1a);
        assert_eq!(payload[1], 0x1a);
        assert_eq!(payload[2], 0x00);
    }
}
