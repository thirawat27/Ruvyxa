//! Native presence and shared-state rooms for real-time collaboration.
//!
//! Rooms are ephemeral and process-local. The server is the single sequencer:
//! every accepted write takes the next room version, so "last writer wins"
//! means "last frame to reach this process wins" and no client clock is
//! involved. Concurrent writes to the same key do not merge — the later
//! arrival replaces the earlier one, and both peers observe the same result
//! because both learn it from the same broadcast.
//!
//! Presence is separate from shared state on purpose. Cursor movement is high
//! frequency and worthless once stale, so it is never retained past the
//! connection that produced it; shared state is retained for the life of the
//! room so a late joiner sees the current document.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::broadcast;

/// Peers allowed in one room. Presence fan-out is O(peers) per update, so this
/// bounds the work a single cursor move can cause.
pub const MAX_ROOM_PEERS: usize = 64;
/// Keys retained in one room's shared state.
pub const MAX_STATE_KEYS: usize = 256;
/// Largest inbound frame accepted from a client.
pub const MAX_FRAME_BYTES: usize = 32 * 1024;
/// Longest shared-state key, in bytes.
pub const MAX_KEY_BYTES: usize = 128;
/// Keys one `set` frame may carry.
pub const MAX_ENTRIES_PER_WRITE: usize = 32;
/// Rooms tracked per process.
pub const MAX_ROOMS: usize = 1024;
/// Inbound frames accepted per connection, per second.
pub const MAX_FRAMES_PER_SECOND: u32 = 120;
/// Buffered broadcast frames before a slow peer is told to resynchronize.
const ROOM_BROADCAST_CAPACITY: usize = 256;

/// A frame sent by a connected client.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ClientBody {
    /// Replace this peer's ephemeral presence state.
    Presence { state: Value },
    /// Write shared-state keys. A `null` value deletes the key.
    Set { entries: Map<String, Value> },
}

#[derive(Debug, Deserialize)]
struct RawClientFrame {
    version: u8,
    #[serde(flatten)]
    body: ClientBody,
}

/// One shared-state key, with the room version that last wrote it.
#[derive(Debug, Clone)]
struct StateEntry {
    value: Value,
    version: u64,
    peer: String,
}

struct Room {
    peers: HashMap<String, Value>,
    state: HashMap<String, StateEntry>,
    version: u64,
    tx: broadcast::Sender<String>,
}

/// A peer's seat in a room: its identity, its feed, and the snapshot it joined on.
pub struct JoinedPeer {
    pub peer: String,
    pub welcome: String,
    pub receiver: broadcast::Receiver<String>,
}

/// Process-local room registry shared by every collaboration connection.
#[derive(Clone, Default)]
pub struct CollabRegistry {
    rooms: Arc<Mutex<HashMap<String, Room>>>,
    next_peer: Arc<AtomicU64>,
}

impl CollabRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Lock the room table, recovering a poisoned mutex instead of panicking.
    ///
    /// Every mutation under this lock is a single map insert, remove, or
    /// replace: there is no invariant spanning two fields that a panic could
    /// leave half-applied, so the state behind a poisoned lock is as valid as
    /// the state behind a healthy one. Propagating the poison instead — which
    /// `.expect("collab registry poisoned")` did at all five call sites — turned
    /// one panic anywhere in the module into a permanent outage: every
    /// subsequent join, presence update, write, and leave panicked in turn, so
    /// collaboration stayed dead for the life of the process and peers could
    /// not even leave a room cleanly.
    ///
    /// The action rate limiter in `lib.rs` makes the opposite call and answers
    /// 503, because refusing an action is safe. Refusing a `leave` is not: it
    /// strands the peer in the room forever.
    fn rooms(&self) -> MutexGuard<'_, HashMap<String, Room>> {
        self.rooms.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Seat a new peer, creating the room when it is the first arrival.
    ///
    /// The welcome frame carries the full room snapshot so a late joiner never
    /// has to replay history, and the announcement is broadcast to peers that
    /// were already seated.
    pub fn join(&self, room_id: &str) -> Result<JoinedPeer, &'static str> {
        if !valid_room_id(room_id) {
            return Err(
                "Collaboration rooms use 1-128 letters, digits, colon, dot, underscore, slash, or dash",
            );
        }
        let mut rooms = self.rooms();
        if !rooms.contains_key(room_id) && rooms.len() >= MAX_ROOMS {
            return Err("Collaboration room limit reached for this server");
        }
        let room = rooms.entry(room_id.to_string()).or_insert_with(|| Room {
            peers: HashMap::new(),
            state: HashMap::new(),
            version: 0,
            tx: broadcast::channel(ROOM_BROADCAST_CAPACITY).0,
        });
        if room.peers.len() >= MAX_ROOM_PEERS {
            // Drop the room again when this rejection created it, so a rejected
            // join cannot leak an empty room into the registry.
            if room.peers.is_empty() {
                rooms.remove(room_id);
            }
            return Err("Collaboration room is full");
        }
        let peer = format!("p{}", self.next_peer.fetch_add(1, Ordering::Relaxed));
        // Subscribe before inserting so this peer cannot miss a frame published
        // between its snapshot and its first read.
        let receiver = room.tx.subscribe();
        let welcome = json!({
            "version": 1,
            "type": "welcome",
            "room": room_id,
            "peer": peer,
            "peers": room.peers.clone(),
            "state": state_payload(&room.state),
            "roomVersion": room.version,
        })
        .to_string();
        room.peers.insert(peer.clone(), Value::Null);
        // The joiner is already subscribed, so it receives its own arrival too.
        // Clients identify themselves from the welcome frame's `peer` and skip
        // frames they authored.
        let _ = room.tx.send(
            json!({
                "version": 1,
                "type": "join",
                "peer": peer,
                "state": Value::Null,
            })
            .to_string(),
        );
        Ok(JoinedPeer {
            peer,
            welcome,
            receiver,
        })
    }

    /// Replace a peer's presence state and tell the room.
    pub fn update_presence(&self, room_id: &str, peer: &str, state: Value) {
        let mut rooms = self.rooms();
        let Some(room) = rooms.get_mut(room_id) else {
            return;
        };
        if !room.peers.contains_key(peer) {
            return;
        }
        room.peers.insert(peer.to_string(), state.clone());
        let _ = room.tx.send(
            json!({
                "version": 1,
                "type": "presence",
                "peer": peer,
                "state": state,
            })
            .to_string(),
        );
    }

    /// Apply a last-writer-wins batch to the room's shared state.
    ///
    /// The whole batch takes one room version, so a client that writes several
    /// keys together sees them land together rather than interleaved with
    /// another peer's write.
    pub fn write_state(
        &self,
        room_id: &str,
        peer: &str,
        entries: Map<String, Value>,
    ) -> Result<(), &'static str> {
        if entries.is_empty() {
            return Ok(());
        }
        if entries.len() > MAX_ENTRIES_PER_WRITE {
            return Err("Collaboration writes carry at most 32 keys");
        }
        if entries.keys().any(|key| !valid_state_key(key)) {
            return Err("Collaboration state keys are 1-128 bytes and cannot be blank");
        }
        let mut rooms = self.rooms();
        let Some(room) = rooms.get_mut(room_id) else {
            return Ok(());
        };
        if !room.peers.contains_key(peer) {
            return Ok(());
        }
        let additions = entries
            .iter()
            .filter(|(key, value)| !value.is_null() && !room.state.contains_key(*key))
            .count();
        if room.state.len() + additions > MAX_STATE_KEYS {
            return Err("Collaboration room holds at most 256 shared-state keys");
        }
        room.version += 1;
        let version = room.version;
        let mut applied = Map::new();
        for (key, value) in entries {
            if value.is_null() {
                room.state.remove(&key);
            } else {
                room.state.insert(
                    key.clone(),
                    StateEntry {
                        value: value.clone(),
                        version,
                        peer: peer.to_string(),
                    },
                );
            }
            applied.insert(key, value);
        }
        let _ = room.tx.send(
            json!({
                "version": 1,
                "type": "patch",
                "peer": peer,
                "roomVersion": version,
                "entries": applied,
            })
            .to_string(),
        );
        Ok(())
    }

    /// Remove a peer and drop the room once the last one leaves.
    pub fn leave(&self, room_id: &str, peer: &str) {
        let mut rooms = self.rooms();
        let Some(room) = rooms.get_mut(room_id) else {
            return;
        };
        if room.peers.remove(peer).is_none() {
            return;
        }
        let _ = room.tx.send(
            json!({
                "version": 1,
                "type": "leave",
                "peer": peer,
            })
            .to_string(),
        );
        if room.peers.is_empty() {
            rooms.remove(room_id);
        }
    }

    #[cfg(test)]
    fn room_count(&self) -> usize {
        self.rooms().len()
    }
}

fn state_payload(state: &HashMap<String, StateEntry>) -> Value {
    let mut payload = Map::new();
    for (key, entry) in state {
        payload.insert(
            key.clone(),
            json!({
                "value": entry.value,
                "version": entry.version,
                "peer": entry.peer,
            }),
        );
    }
    Value::Object(payload)
}

/// Decode one inbound frame, rejecting anything a client should not be able to
/// make the server hold onto.
pub fn parse_client_frame(payload: &str) -> Result<ParsedFrame, &'static str> {
    if payload.len() > MAX_FRAME_BYTES {
        return Err("Collaboration frames are limited to 32 KiB");
    }
    let frame: RawClientFrame =
        serde_json::from_str(payload).map_err(|_| "Collaboration frame is not valid JSON")?;
    if frame.version != 1 {
        return Err("Collaboration frame version is not supported");
    }
    Ok(match frame.body {
        ClientBody::Presence { state } => ParsedFrame::Presence(state),
        ClientBody::Set { entries } => ParsedFrame::Set(entries),
    })
}

/// An inbound frame that passed decoding.
#[derive(Debug)]
pub enum ParsedFrame {
    Presence(Value),
    Set(Map<String, Value>),
}

pub fn valid_room_id(room: &str) -> bool {
    !room.is_empty()
        && room.len() <= 128
        && room.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'.' | b'_' | b'/' | b'-')
        })
}

fn valid_state_key(key: &str) -> bool {
    !key.trim().is_empty() && key.len() <= MAX_KEY_BYTES
}

/// Fixed-window frame limiter applied per connection.
///
/// Cursor streams are bursty, so the window is deliberately coarse: a peer may
/// spend its whole budget in one frame of animation without being throttled,
/// but cannot sustain more than the cap.
pub struct FrameRateLimiter {
    window_start: Instant,
    count: u32,
}

impl FrameRateLimiter {
    pub fn new(now: Instant) -> Self {
        Self {
            window_start: now,
            count: 0,
        }
    }

    pub fn allow(&mut self, now: Instant) -> bool {
        if now.duration_since(self.window_start) >= Duration::from_secs(1) {
            self.window_start = now;
            self.count = 0;
        }
        self.count += 1;
        self.count <= MAX_FRAMES_PER_SECOND
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn a_poisoned_registry_keeps_serving_rooms() {
        // One panic while the lock was held used to end collaboration for the
        // life of the process: every later call hit `.expect("collab registry
        // poisoned")` and panicked in turn, so peers could not even leave.
        let registry = CollabRegistry::new();
        let joined = registry.join("room").expect("first join succeeds");

        let poisoner = registry.clone();
        std::thread::spawn(move || {
            let _guard = poisoner.rooms();
            panic!("poison the registry");
        })
        .join()
        .expect_err("the spawned thread must panic");

        // Every entry point still works on the state left behind.
        registry.update_presence("room", &joined.peer, json!({"cursor": 1}));
        registry
            .write_state("room", &joined.peer, entries(&[("k", json!("v"))]))
            .expect("writes still land after poisoning");
        let second = registry.join("room").expect("joins still succeed");
        registry.leave("room", &joined.peer);
        registry.leave("room", &second.peer);
        assert_eq!(
            registry.room_count(),
            0,
            "the last peer leaving still drops the room"
        );
    }

    fn decode(payload: &str) -> Value {
        serde_json::from_str(payload).expect("broadcast frame is JSON")
    }

    #[test]
    fn join_returns_a_snapshot_and_announces_the_peer() {
        let registry = CollabRegistry::new();
        let first = registry.join("doc:1").expect("first join");
        let mut first_feed = first.receiver;
        // The joiner's own announcement is on its feed; drain it so the next
        // read is the second peer's arrival.
        assert_eq!(decode(&first_feed.try_recv().unwrap())["type"], "join");

        registry
            .write_state("doc:1", &first.peer, entries(&[("title", json!("Draft"))]))
            .expect("write");
        assert_eq!(decode(&first_feed.try_recv().unwrap())["type"], "patch");

        let second = registry.join("doc:1").expect("second join");
        let welcome = decode(&second.welcome);
        assert_eq!(welcome["type"], "welcome");
        assert_eq!(welcome["room"], "doc:1");
        assert_eq!(welcome["state"]["title"]["value"], "Draft");
        assert_eq!(welcome["state"]["title"]["peer"], first.peer.as_str());
        assert_eq!(welcome["roomVersion"], 1);
        // The snapshot lists the peer that was already seated, not the joiner.
        assert!(welcome["peers"].get(&first.peer).is_some());
        assert!(welcome["peers"].get(&second.peer).is_none());

        let announced = decode(&first_feed.try_recv().unwrap());
        assert_eq!(announced["type"], "join");
        assert_eq!(announced["peer"], second.peer.as_str());
    }

    #[test]
    fn the_later_write_wins_and_both_peers_see_the_same_value() {
        let registry = CollabRegistry::new();
        let first = registry.join("doc:2").expect("first join");
        let second = registry.join("doc:2").expect("second join");
        let mut feed = second.receiver;

        registry
            .write_state("doc:2", &first.peer, entries(&[("title", json!("A"))]))
            .expect("first write");
        registry
            .write_state("doc:2", &second.peer, entries(&[("title", json!("B"))]))
            .expect("second write");

        let mut last = Value::Null;
        while let Ok(frame) = feed.try_recv() {
            let frame = decode(&frame);
            if frame["type"] == "patch" {
                last = frame;
            }
        }
        assert_eq!(last["entries"]["title"], "B");
        assert_eq!(last["roomVersion"], 2);

        // A peer joining now reads the same winner from the snapshot, so the
        // broadcast and the snapshot cannot disagree.
        let third = registry.join("doc:2").expect("third join");
        assert_eq!(decode(&third.welcome)["state"]["title"]["value"], "B");
    }

    #[test]
    fn a_null_value_deletes_the_key() {
        let registry = CollabRegistry::new();
        let peer = registry.join("doc:3").expect("join");
        registry
            .write_state("doc:3", &peer.peer, entries(&[("draft", json!(true))]))
            .expect("write");
        registry
            .write_state("doc:3", &peer.peer, entries(&[("draft", Value::Null)]))
            .expect("delete");
        let next = registry.join("doc:3").expect("join");
        assert!(decode(&next.welcome)["state"].get("draft").is_none());
    }

    #[test]
    fn presence_is_dropped_with_the_peer_but_shared_state_survives() {
        let registry = CollabRegistry::new();
        let first = registry.join("doc:4").expect("first join");
        let second = registry.join("doc:4").expect("second join");
        registry.update_presence("doc:4", &first.peer, json!({ "cursor": [10, 20] }));
        registry
            .write_state("doc:4", &first.peer, entries(&[("title", json!("Kept"))]))
            .expect("write");
        registry.leave("doc:4", &first.peer);

        let third = registry.join("doc:4").expect("third join");
        let welcome = decode(&third.welcome);
        assert!(welcome["peers"].get(&first.peer).is_none());
        assert!(welcome["peers"].get(&second.peer).is_some());
        assert_eq!(welcome["state"]["title"]["value"], "Kept");
    }

    #[test]
    fn the_room_is_dropped_once_the_last_peer_leaves() {
        let registry = CollabRegistry::new();
        let peer = registry.join("doc:5").expect("join");
        registry
            .write_state("doc:5", &peer.peer, entries(&[("title", json!("Gone"))]))
            .expect("write");
        assert_eq!(registry.room_count(), 1);
        registry.leave("doc:5", &peer.peer);
        assert_eq!(registry.room_count(), 0);

        // The next room with the same id starts empty rather than resurrecting
        // the previous document.
        let fresh = registry.join("doc:5").expect("rejoin");
        assert_eq!(decode(&fresh.welcome)["state"], json!({}));
        assert_eq!(decode(&fresh.welcome)["roomVersion"], 0);
    }

    #[test]
    fn writes_from_a_peer_that_left_are_ignored() {
        let registry = CollabRegistry::new();
        let first = registry.join("doc:6").expect("first join");
        let second = registry.join("doc:6").expect("second join");
        registry.leave("doc:6", &first.peer);
        registry
            .write_state("doc:6", &first.peer, entries(&[("title", json!("Stale"))]))
            .expect("ignored write");
        let third = registry.join("doc:6").expect("third join");
        assert!(decode(&third.welcome)["state"].get("title").is_none());
        drop(second);
    }

    #[test]
    fn room_and_key_limits_are_enforced() {
        let registry = CollabRegistry::new();
        assert!(registry.join("").is_err());
        assert!(registry.join(&"a".repeat(129)).is_err());
        assert!(registry.join("doc 7").is_err());
        // A rejected join must not leave an empty room behind.
        assert_eq!(registry.room_count(), 0);

        let peer = registry.join("doc:7").expect("join");
        assert!(
            registry
                .write_state("doc:7", &peer.peer, entries(&[(" ", json!(1))]))
                .is_err()
        );
        let oversized = (0..=MAX_ENTRIES_PER_WRITE)
            .map(|index| (format!("k{index}"), json!(index)))
            .collect::<Map<String, Value>>();
        assert!(
            registry
                .write_state("doc:7", &peer.peer, oversized)
                .is_err()
        );
    }

    #[test]
    fn a_full_room_rejects_further_peers() {
        let registry = CollabRegistry::new();
        let seated = (0..MAX_ROOM_PEERS)
            .map(|_| registry.join("doc:8").expect("join"))
            .collect::<Vec<_>>();
        assert!(registry.join("doc:8").is_err());
        registry.leave("doc:8", &seated[0].peer);
        assert!(registry.join("doc:8").is_ok());
    }

    #[test]
    fn client_frames_are_validated_before_they_reach_a_room() {
        assert!(matches!(
            parse_client_frame(r#"{"version":1,"type":"presence","state":{"cursor":[1,2]}}"#),
            Ok(ParsedFrame::Presence(_))
        ));
        assert!(matches!(
            parse_client_frame(r#"{"version":1,"type":"set","entries":{"title":"A"}}"#),
            Ok(ParsedFrame::Set(_))
        ));
        assert!(parse_client_frame(r#"{"version":2,"type":"presence","state":{}}"#).is_err());
        assert!(parse_client_frame(r#"{"version":1,"type":"evict","peer":"p0"}"#).is_err());
        assert!(parse_client_frame("not json").is_err());
        let oversized = format!(
            r#"{{"version":1,"type":"presence","state":"{}"}}"#,
            "x".repeat(MAX_FRAME_BYTES)
        );
        assert!(parse_client_frame(&oversized).is_err());
    }

    #[test]
    fn the_frame_limiter_resets_each_window() {
        let start = Instant::now();
        let mut limiter = FrameRateLimiter::new(start);
        for _ in 0..MAX_FRAMES_PER_SECOND {
            assert!(limiter.allow(start));
        }
        assert!(!limiter.allow(start));
        assert!(limiter.allow(start + Duration::from_secs(1)));
    }
}
