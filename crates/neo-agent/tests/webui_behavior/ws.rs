//! WebSocket client helper for the web long connection: connect with the
//! exact Host/Origin/Cookie headers, send a `watch_session` frame and read the
//! first JSON (snapshot or envelope).

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub(crate) type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub(crate) struct Watch {
    pub(crate) read: futures::stream::SplitStream<WsStream>,
    pub(crate) write: futures::stream::SplitSink<WsStream, Message>,
}

impl Watch {
    pub(crate) async fn next_json(&mut self) -> Value {
        loop {
            match tokio::time::timeout(Duration::from_secs(10), self.read.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    return serde_json::from_str(&text).expect("ws json");
                }
                Ok(Some(Ok(_))) => continue,
                Ok(Some(Err(error))) => panic!("ws read error: {error}"),
                Ok(None) => panic!("ws closed unexpectedly"),
                Err(_) => panic!("ws read timed out"),
            }
        }
    }
}

/// Connect to `/api/events`, watch one session and return the first payload
/// (a full snapshot for a fresh cursor, or the first replay envelope).
pub(crate) async fn connect_watch(
    port: u16,
    cookie: &str,
    session_id: &str,
    after: Option<(String, u64)>,
) -> (Watch, Value) {
    let request = http::Request::builder()
        .uri(format!("ws://127.0.0.1:{port}/api/events"))
        .header("Host", format!("127.0.0.1:{port}"))
        .header("Origin", format!("http://127.0.0.1:{port}"))
        .header("Cookie", cookie)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .expect("ws request");
    let (socket, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .expect("ws connect deadline")
    .expect("ws handshake");
    let (write, read) = socket.split();
    let mut watch = Watch { read, write };
    let frame = match after {
        None => json!({ "type": "watch_session", "session_id": session_id, "after": null }),
        Some((stream_id, sequence)) => json!({
            "type": "watch_session",
            "session_id": session_id,
            "after": { "stream_id": stream_id, "sequence": sequence }
        }),
    };
    watch
        .write
        .send(Message::Text(frame.to_string()))
        .await
        .expect("send watch frame");
    let first = watch.next_json().await;
    (watch, first)
}
