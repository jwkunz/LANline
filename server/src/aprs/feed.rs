//! TNC2-format APRS monitor feed: fan decoded packet lines out over TCP.
//! Point Xastir / YAAC / an APRS-IS uploader at it (read-only text stream,
//! `SRC>DEST,PATH:info` CRLF-terminated). Mirrors `crate::ais::nmea::serve`.

use bytes::Bytes;
use std::net::{IpAddr, SocketAddr};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

/// Bind the feed and fan `tx` out to every client. `port == 0` disables it.
/// Returns the bound port (0 when disabled).
pub fn serve(bind: IpAddr, port: u16, tx: broadcast::Sender<Bytes>) -> u16 {
    if port == 0 {
        return 0;
    }
    let listener = match std::net::TcpListener::bind(SocketAddr::new(bind, port)) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("aprs: cannot bind tcp/{port}: {e}; TNC2 feed disabled");
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
                tracing::warn!("aprs: listener adopt failed: {e}");
                return;
            }
        };
        tracing::info!("aprs: tcp/{bound} ready (TNC2 feed)");
        loop {
            let Ok((mut sock, peer)) = listener.accept().await else {
                continue;
            };
            let _ = sock.set_nodelay(true);
            let mut rx = tx.subscribe();
            tokio::spawn(async move {
                tracing::debug!("aprs: client {peer} connected");
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
                tracing::debug!("aprs: client {peer} gone");
            });
        }
    });

    bound
}
