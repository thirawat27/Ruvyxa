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
    ws.on_upgrade(move |mut socket| async move {
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
    ws.on_upgrade(move |mut socket| async move {
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
/// is validated, rate limited, and applied to process-local room state before
/// it is fanned out, so a peer can never make the server retain unbounded data
/// or forward a frame the room's own encoder did not produce.
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
    ws.on_upgrade(move |socket| async move {
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
