use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

static NEXT_EDIT: AtomicU64 = AtomicU64::new(1);
const TRACE_LIMIT: usize = 128;

/// Stages one trace keeps in full before it starts counting instead.
///
/// The store was bounded in traces but not in events per trace, and `record` is
/// reachable from `/__ruvyxa/trace-ack` with a caller-supplied id: one trace
/// could be grown without limit by request volume, and `snapshot` clones every
/// event on each DevTools poll, so the endpoint got steadily more expensive as
/// it did. A real edit produces a handful of stages; anything past this many is
/// a client repeating itself, and the count says so without keeping the bytes.
const TRACE_EVENT_LIMIT: usize = 64;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraceEvent {
    stage: String,
    at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditTrace {
    trace_id: String,
    paths: Vec<String>,
    routes: Vec<String>,
    kind: String,
    events: Vec<TraceEvent>,
    /// Stages dropped because this trace had already reached
    /// [`TRACE_EVENT_LIMIT`]. Kept as a count so a truncated timeline says it is
    /// truncated instead of quietly ending early. Absent when nothing was lost.
    #[serde(skip_serializing_if = "is_zero")]
    suppressed_events: u64,
}

fn is_zero(count: &u64) -> bool {
    *count == 0
}

/// Bounded, process-local edit traces used by dev diagnostics.
#[derive(Default)]
pub(crate) struct TraceStore {
    traces: Mutex<VecDeque<EditTrace>>,
}

impl TraceStore {
    pub(crate) fn start(&self, trace_id: &str, paths: &[String], routes: &[String], kind: &str) {
        let mut traces = self
            .traces
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while traces.len() >= TRACE_LIMIT {
            traces.pop_front();
        }
        traces.push_back(EditTrace {
            trace_id: trace_id.to_string(),
            paths: paths.to_vec(),
            routes: routes.to_vec(),
            kind: kind.to_string(),
            events: vec![event("graph", Some("edit accepted"))],
            suppressed_events: 0,
        });
    }

    pub(crate) fn record(&self, trace_id: &str, stage: &str, detail: impl Into<String>) -> bool {
        let detail = detail.into();
        let mut traces = self
            .traces
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(trace) = traces.iter_mut().find(|trace| trace.trace_id == trace_id) {
            // Past the cap the stage is counted rather than kept. The answer is
            // still `true`: whether this id exists is what the endpoint's
            // 404-on-unknown-id semantics turn on, and it does exist.
            if trace.events.len() >= TRACE_EVENT_LIMIT {
                trace.suppressed_events += 1;
            } else {
                trace.events.push(event(stage, Some(&detail)));
            }
            return true;
        }
        false
    }

    pub(crate) fn snapshot(&self, path: Option<&str>) -> Vec<EditTrace> {
        let traces = self
            .traces
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        traces
            .iter()
            .filter(|trace| path.is_none_or(|path| trace.paths.iter().any(|item| item == path)))
            .cloned()
            .collect()
    }
}

fn event(stage: &str, detail: Option<&str>) -> TraceEvent {
    TraceEvent {
        stage: stage.to_string(),
        at_ms: now_ms(),
        detail: detail.map(str::to_string),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// Create a W3C-sized trace identifier for one file-system edit event.
pub(crate) fn edit_id(paths: &[String]) -> String {
    let mut hash = blake3::Hasher::new();
    hash.update(&std::process::id().to_le_bytes());
    hash.update(&NEXT_EDIT.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    hash.update(
        &SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    for path in paths {
        hash.update(path.as_bytes());
        hash.update(&[0]);
    }
    hash.finalize().to_hex()[..32].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_ids_are_w3c_sized_hex_and_unique() {
        let paths = vec!["app/page.tsx".to_string()];
        let first = edit_id(&paths);
        let second = edit_id(&paths);
        assert_eq!(first.len(), 32);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn store_is_bounded_and_filters_by_path() {
        let store = TraceStore::default();
        for index in 0..=TRACE_LIMIT {
            store.start(
                &format!("{index:032x}"),
                &[format!("app/{index}.tsx")],
                &[format!("/{index}")],
                "server-route",
            );
        }
        assert_eq!(store.snapshot(None).len(), TRACE_LIMIT);
        assert!(store.snapshot(Some("app/0.tsx")).is_empty());
        assert_eq!(store.snapshot(Some("app/128.tsx")).len(), 1);
        assert!(store.record(&format!("{:032x}", TRACE_LIMIT), "browser", "received"));
        assert!(!store.record("ffffffffffffffffffffffffffffffff", "browser", "missing"));
    }

    /// `record` is reachable from an HTTP endpoint with a caller-supplied id, so
    /// one trace must not grow without limit — and a truncated trace must say so
    /// rather than simply ending.
    #[test]
    fn one_trace_bounds_its_own_events() {
        let store = TraceStore::default();
        let trace_id = "0".repeat(32);
        store.start(
            &trace_id,
            &["app/page.tsx".to_string()],
            &["/".to_string()],
            "page",
        );

        for index in 0..TRACE_EVENT_LIMIT * 4 {
            assert!(
                store.record(&trace_id, "browser", format!("ack {index}")),
                "a known id stays known however many stages it has already seen"
            );
        }

        let traces = store.snapshot(None);
        let trace = traces.first().expect("the trace is still stored");
        assert_eq!(trace.events.len(), TRACE_EVENT_LIMIT);
        // The `start` event holds one of the slots, so every push past the
        // remaining ones is counted.
        assert_eq!(
            trace.suppressed_events,
            (TRACE_EVENT_LIMIT * 4 - (TRACE_EVENT_LIMIT - 1)) as u64
        );
        let rendered = serde_json::to_string(trace).expect("a trace serializes");
        assert!(
            rendered.contains("suppressedEvents"),
            "a truncated trace must report the loss: {rendered}"
        );
    }
}
