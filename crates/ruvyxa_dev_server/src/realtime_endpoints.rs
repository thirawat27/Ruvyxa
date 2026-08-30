//! The three WebSocket endpoints: HMR, realtime, and presence.
//!
//! Each is an upgrade handler plus the frame rules that go with it — origin
//! checking on the handshake, channel parsing and validation, per-connection
//! rate limiting, and the subscription filter deciding which broadcast frames a
//! socket is entitled to. They sat in the crate root among the HTTP handlers,
//! where the shared filtering rules read as loose helpers rather than as one
//! endpoint family.
//!
//! `lib.rs` still owns the router that mounts them; what a socket is allowed to
//! see lives here.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio::sync::broadcast;

use crate::AppState;
use crate::action_security::hmr_origin_is_cross_site;
use crate::collab::{self, FrameRateLimiter, ParsedFrame, parse_client_frame};
#[cfg(test)]
use crate::render_pipeline::decode_realtime_event;
use crate::response::with_security_headers;

/// Bound the transport to the size the frame parser accepts, before a byte of
/// payload is read.
///
/// tungstenite defaults to a 64 MiB message and a 16 MiB frame, and
/// [`parse_client_frame`] compared against [`collab::MAX_FRAME_BYTES`] only
/// once the whole message was already a `String` in this process — as did the
/// per-connection [`FrameRateLimiter`], which therefore throttled frames and
/// not allocation. An unauthenticated peer could force 64 MiB per socket per
/// message on the host that also serves the application, multiplied by
/// concurrent connections: `MAX_ROOM_PEERS` and `MAX_ROOMS` bound peers that
/// have been *seated*, not sockets still sending their first frame.
///
/// Both bounds are derived from the parser's own constant so the transport
/// bound and the parser bound cannot drift apart. The realtime and HMR sockets
/// are write-only, so the same bound costs them nothing and means a socket that
/// grows a read later starts out bounded.
///
/// A peer sending between the limit and 64 MiB used to get a JSON `error` frame
/// and keep its connection; now tungstenite fails the frame during header
/// parsing and the socket ends. That is the correct answer for a peer that
/// ignored a documented limit, and it is the only one that can be given before
/// the payload exists.
fn bounded_upgrade(ws: WebSocketUpgrade) -> WebSocketUpgrade {
    ws.max_message_size(collab::MAX_FRAME_BYTES)
        .max_frame_size(collab::MAX_FRAME_BYTES)
}

pub(crate) async fn hmr_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    // Cross-site pages can open WebSockets to localhost and, unlike fetch,
    // read the messages. Reject handshakes without same-origin evidence so
    // project structure never leaks to other sites open in the browser.
    if hmr_origin_is_cross_site(&headers, &state.config, peer.ip()) {
        return with_security_headers(
            (StatusCode::FORBIDDEN, "Cross-origin HMR connection blocked").into_response(),
        );
    }
    bounded_upgrade(ws).on_upgrade(move |mut socket| async move {
        let mut reload_rx = state.reload_tx.subscribe();

        while let Ok(payload) = reload_rx.recv().await {
            if socket.send(Message::Text(payload.into())).await.is_err() {
                break;
            }
        }
    })
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct RealtimeQuery {
    channels: Option<String>,
}

pub(crate) async fn realtime_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(query): Query<RealtimeQuery>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let Some(runtime) = state.realtime.clone() else {
        return (StatusCode::NOT_FOUND, "Realtime is not enabled").into_response();
    };
    if hmr_origin_is_cross_site(&headers, &state.config, peer.ip()) {
        return with_security_headers(
            (
                StatusCode::FORBIDDEN,
                "Cross-origin realtime connection blocked",
            )
                .into_response(),
        );
    }
    let channels = match parse_realtime_channels(query.channels.as_deref()) {
        Ok(channels) => channels,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    bounded_upgrade(ws).on_upgrade(move |mut socket| async move {
        let mut receiver = runtime.tx.subscribe();
        let mut heartbeat = tokio::time::interval(runtime.heartbeat);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                received = receiver.recv() => {
                    match received {
                        Ok(payload) if realtime_payload_matches(&payload, &channels) => {
                            if socket.send(Message::Text(payload.into())).await.is_err() {
                                break;
                            }
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            if socket.send(Message::Text(
                                r#"{"version":1,"type":"resync","reason":"lagged"}"#.into(),
                            )).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = heartbeat.tick() => {
                    if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    })
}

#[derive(Deserialize)]
pub(crate) struct PresenceQuery {
    room: Option<String>,
}

/// Serve one collaboration room membership over a bidirectional socket.
///
/// Unlike the realtime transport, this socket reads. Everything a client sends
/// is size-bounded by the transport, then validated, rate limited, and applied
/// to process-local room state before it is fanned out, so a peer can never
/// make the server hold or retain unbounded data, or forward a frame the room's
/// own encoder did not produce.
///
/// The first clause used to be true only of *retained* state: the frame limit
/// and the rate limiter both ran on a message the transport had already
/// buffered in full. [`bounded_upgrade`] is what makes it true of the buffer.
pub(crate) async fn presence_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(query): Query<PresenceQuery>,
    axum::extract::ConnectInfo(peer_addr): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let Some(runtime) = state.presence.clone() else {
        return (StatusCode::NOT_FOUND, "Presence is not enabled").into_response();
    };
    if hmr_origin_is_cross_site(&headers, &state.config, peer_addr.ip()) {
        return with_security_headers(
            (
                StatusCode::FORBIDDEN,
                "Cross-origin presence connection blocked",
            )
                .into_response(),
        );
    }
    let Some(room) = query.room.filter(|room| collab::valid_room_id(room)) else {
        return (
            StatusCode::BAD_REQUEST,
            "Presence requires a room of 1-128 letters, digits, colon, dot, underscore, slash, or dash",
        )
            .into_response();
    };
    bounded_upgrade(ws).on_upgrade(move |socket| async move {
        use futures_util::{SinkExt, StreamExt};

        let registry = runtime.registry;
        let (mut sender, mut receiver) = socket.split();
        // Seat the peer only once the upgrade succeeded, so a handshake that
        // never completes cannot leave an occupant behind in the room.
        let seat = match registry.join(&room) {
            Ok(seat) => seat,
            Err(message) => {
                let _ = sender.send(Message::Text(error_frame(message).into())).await;
                let _ = sender.close().await;
                return;
            }
        };
        if sender
            .send(Message::Text(seat.welcome.clone().into()))
            .await
            .is_err()
        {
            registry.leave(&room, &seat.peer);
            return;
        }
        let mut feed = seat.receiver;
        let mut heartbeat = tokio::time::interval(runtime.heartbeat);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut limiter = FrameRateLimiter::new(Instant::now());

        loop {
            tokio::select! {
                broadcast = feed.recv() => {
                    match broadcast {
                        Ok(payload) => {
                            if sender.send(Message::Text(payload.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            // The peer fell behind the room's frame buffer, so
                            // its view is now unreconstructable from the feed
                            // alone; tell it to reconnect for a fresh snapshot.
                            let _ = sender.send(Message::Text(
                                r#"{"version":1,"type":"resync","reason":"lagged"}"#.into(),
                            )).await;
                            break;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                inbound = receiver.next() => {
                    let Some(Ok(message)) = inbound else { break };
                    let Message::Text(payload) = message else { continue };
                    if !limiter.allow(Instant::now()) {
                        let _ = sender.send(Message::Text(
                            error_frame("Collaboration frame rate exceeded").into(),
                        )).await;
                        break;
                    }
                    let frame = match parse_client_frame(&payload) {
                        Ok(frame) => frame,
                        Err(message) => {
                            if sender.send(Message::Text(error_frame(message).into())).await.is_err() {
                                break;
                            }
                            continue;
                        }
                    };
                    match frame {
                        ParsedFrame::Presence(presence) => {
                            registry.update_presence(&room, &seat.peer, presence);
                        }
                        ParsedFrame::Set(entries) => {
                            if let Err(message) = registry.write_state(&room, &seat.peer, entries)
                                && sender.send(Message::Text(error_frame(message).into())).await.is_err()
                            {
                                break;
                            }
                        }
                    }
                }
                _ = heartbeat.tick() => {
                    if sender.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
        registry.leave(&room, &seat.peer);
    })
}

fn error_frame(message: &str) -> String {
    serde_json::json!({ "version": 1, "type": "error", "message": message }).to_string()
}

fn parse_realtime_channels(
    value: Option<&str>,
) -> std::result::Result<HashSet<String>, &'static str> {
    let Some(value) = value else {
        return Err("Realtime requires at least one channel");
    };
    let requested = value.split(',').collect::<Vec<_>>();
    if requested.is_empty()
        || requested.len() > 16
        || requested.iter().any(|channel| channel.is_empty())
    {
        return Err("Realtime requires between 1 and 16 non-empty channels");
    }
    let channels = requested
        .into_iter()
        .map(str::to_string)
        .collect::<HashSet<_>>();
    if channels.len() > 16
        || channels
            .iter()
            .any(|channel| !valid_realtime_channel(channel))
    {
        return Err("Realtime accepts at most 16 channels of 128 bytes each");
    }
    Ok(channels)
}

pub(crate) fn valid_realtime_channel(channel: &str) -> bool {
    !channel.is_empty()
        && channel.len() <= 128
        && channel.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'.' | b'_' | b'/' | b'-')
        })
}

fn realtime_payload_matches(payload: &str, subscriptions: &HashSet<String>) -> bool {
    if subscriptions.is_empty() {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()
        .and_then(|value| {
            value
                .get("channels")
                .and_then(serde_json::Value::as_array)
                .cloned()
        })
        .is_some_and(|channels| {
            channels.iter().any(|channel| {
                channel
                    .as_str()
                    .is_some_and(|channel| subscriptions.contains(channel))
            })
        })
}

#[cfg(test)]
mod tests {
    use base64::Engine;

    use super::*;

    /// Every socket in this module must be bounded before it is upgraded.
    ///
    /// The transport limit is invisible from outside the connection, so nothing
    /// a future socket handler does can fail loudly if it forgets it — it just
    /// inherits tungstenite's 64 MiB default again. The three handlers here and
    /// the one the transport test mounts are checked mechanically instead,
    /// after whitespace is removed so rustfmt may break the call across lines.
    #[test]
    fn every_socket_in_this_module_bounds_the_transport_before_upgrading() {
        let source = include_str!("realtime_endpoints.rs")
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        // Assembled rather than written out, so this line is not itself one of
        // the occurrences the gate then demands be bounded.
        let call = format!(".{}(", "on_upgrade");
        let upgrades = source.match_indices(call.as_str()).collect::<Vec<_>>();
        assert!(
            upgrades.len() >= 4,
            "the gate found no upgrades to check, so it is checking nothing"
        );
        for (index, _) in upgrades {
            assert!(
                source[..index].ends_with("bounded_upgrade(ws)"),
                "a WebSocket upgrade in this module skips bounded_upgrade and \
                 inherits tungstenite's 64 MiB default message size"
            );
        }
    }

    /// A frame larger than the parser accepts is refused by the transport.
    ///
    /// `parse_client_frame` tested `payload.len() > MAX_FRAME_BYTES` only once
    /// the whole message was already a `String` in this process, and the
    /// per-connection rate limiter ran after materialisation too — so neither
    /// bounded allocation at all. An unauthenticated peer could force 64 MiB
    /// per socket per message, multiplied by concurrent connections rather than
    /// by `MAX_ROOM_PEERS`, which bounds only peers that have been seated.
    ///
    /// The small frame comes first so a failure here cannot be the harness
    /// silently delivering nothing.
    #[tokio::test]
    async fn a_frame_over_the_parser_limit_never_reaches_the_handler() {
        use axum::Router;
        use axum::routing::get;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::{TcpListener, TcpStream};
        use tokio::sync::mpsc;

        /// `Some(len)` for each delivered text frame, `None` once the socket
        /// loop ends. A closed channel cannot stand in for the second: the
        /// router owns the sender for as long as the server task lives.
        type Delivered = mpsc::UnboundedSender<Option<usize>>;

        async fn echo_length(ws: WebSocketUpgrade, State(delivered): State<Delivered>) -> Response {
            bounded_upgrade(ws).on_upgrade(move |mut socket| async move {
                while let Some(Ok(Message::Text(text))) = socket.recv().await {
                    if delivered.send(Some(text.len())).is_err() {
                        return;
                    }
                }
                let _ = delivered.send(None);
            })
        }

        /// One masked client text frame. Clients must mask; servers must not.
        fn client_text_frame(payload: &[u8]) -> Vec<u8> {
            let mut frame = vec![0x81];
            match payload.len() {
                length if length < 126 => frame.push(0x80 | length as u8),
                length if length <= usize::from(u16::MAX) => {
                    frame.push(0x80 | 126);
                    frame.extend_from_slice(&(length as u16).to_be_bytes());
                }
                length => {
                    frame.push(0x80 | 127);
                    frame.extend_from_slice(&(length as u64).to_be_bytes());
                }
            }
            let mask = [0x21_u8, 0x5a, 0x0f, 0xc3];
            frame.extend_from_slice(&mask);
            frame.extend(
                payload
                    .iter()
                    .zip((0..4).cycle())
                    .map(|(byte, index): (&u8, usize)| byte ^ mask[index]),
            );
            frame
        }

        let (delivered_tx, mut delivered) = mpsc::unbounded_channel();
        let app = Router::new()
            .route("/", get(echo_length))
            .with_state(delivered_tx);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let mut client = TcpStream::connect(address).await.expect("connect");
        client
            .write_all(
                b"GET / HTTP/1.1\r\n\
                  Host: localhost\r\n\
                  Connection: Upgrade\r\n\
                  Upgrade: websocket\r\n\
                  Sec-WebSocket-Version: 13\r\n\
                  Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
            )
            .await
            .expect("handshake");
        let mut handshake = Vec::new();
        let mut byte = [0_u8; 1];
        while !handshake.ends_with(b"\r\n\r\n") {
            assert_eq!(client.read(&mut byte).await.expect("read"), 1, "handshake");
            handshake.push(byte[0]);
        }
        assert!(
            String::from_utf8_lossy(&handshake).starts_with("HTTP/1.1 101"),
            "the upgrade must succeed before the frame limit means anything"
        );

        let deadline = std::time::Duration::from_secs(10);
        client
            .write_all(&client_text_frame(b"ok"))
            .await
            .expect("small frame");
        assert_eq!(
            tokio::time::timeout(deadline, delivered.recv())
                .await
                .expect("a frame inside the limit is delivered"),
            Some(Some(2))
        );

        let oversize = vec![b'x'; collab::MAX_FRAME_BYTES + 1];
        client
            .write_all(&client_text_frame(&oversize))
            .await
            .expect("oversize frame");
        assert_eq!(
            tokio::time::timeout(deadline, delivered.recv())
                .await
                .expect("the socket must end rather than hang"),
            Some(None),
            "the oversize frame reached the handler, so the transport buffered it"
        );

        server.abort();
    }

    #[test]
    fn validates_realtime_event_metadata_and_channel_filters() {
        let payload = r#"{"version":1,"type":"action","channels":["todos"],"action":"save","path":"/todos","invalidated":[]}"#;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
        assert_eq!(decode_realtime_event(&encoded).unwrap(), payload);
        assert!(decode_realtime_event("not-base64!").is_err());

        let subscriptions = HashSet::from(["todos".to_string()]);
        assert!(realtime_payload_matches(payload, &subscriptions));
        assert!(!realtime_payload_matches(
            r#"{"version":1,"type":"action","channels":["users"]}"#,
            &subscriptions
        ));
        assert!(parse_realtime_channels(Some("todos,users")).is_ok());
        assert!(parse_realtime_channels(None).is_err());
        assert!(parse_realtime_channels(Some("todos,,users")).is_err());
        assert!(parse_realtime_channels(Some(&"a".repeat(129))).is_err());
    }
}
