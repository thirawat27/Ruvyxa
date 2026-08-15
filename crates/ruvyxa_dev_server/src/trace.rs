use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

static NEXT_EDIT: AtomicU64 = AtomicU64::new(1);
const TRACE_LIMIT: usize = 128;

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
        });
    }

    pub(crate) fn record(&self, trace_id: &str, stage: &str, detail: impl Into<String>) -> bool {
        let detail = detail.into();
        let mut traces = self
            .traces
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(trace) = traces.iter_mut().find(|trace| trace.trace_id == trace_id) {
            trace.events.push(event(stage, Some(&detail)));
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
}
