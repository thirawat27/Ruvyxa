//! Shared memory budget for build caches.
//!
//! Cache owners retain their own values and eviction mechanics. This module
//! supplies one hysteresis contract and correlated counters so compiler,
//! resolver, artifact, and worker implementations make the same decision for
//! the same pressure signal.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CachePressureLevel {
    None,
    Soft,
    Hard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachePressureAction {
    pub level: CachePressureLevel,
    pub target_bytes: u64,
    pub to_free_bytes: u64,
    pub stop_speculation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheBudgetSnapshot {
    pub hard_limit_bytes: u64,
    pub soft_limit_bytes: u64,
    pub target_bytes: u64,
    pub resident_bytes: u64,
    pub pressure_level: CachePressureLevel,
    pub pressure_events: u64,
    pub evictions: BTreeMap<String, u64>,
}

#[derive(Debug, Default)]
struct CacheBudgetState {
    pressure_level: Option<CachePressureLevel>,
    pressure_events: u64,
    evictions: BTreeMap<String, u64>,
}

/// Thread-safe pressure policy shared by every cache in one build context.
#[derive(Debug, Clone)]
pub struct CacheBudget {
    hard_limit_bytes: u64,
    soft_limit_bytes: u64,
    target_bytes: u64,
    state: Arc<Mutex<CacheBudgetState>>,
}

impl CacheBudget {
    pub fn new(hard_limit_bytes: u64, soft_ratio: f64, target_ratio: f64) -> Option<Self> {
        if hard_limit_bytes == 0
            || !soft_ratio.is_finite()
            || !target_ratio.is_finite()
            || !(0.0 < target_ratio && target_ratio < soft_ratio && soft_ratio < 1.0)
        {
            return None;
        }
        Some(Self {
            hard_limit_bytes,
            soft_limit_bytes: (hard_limit_bytes as f64 * soft_ratio).floor() as u64,
            target_bytes: (hard_limit_bytes as f64 * target_ratio).floor() as u64,
            state: Arc::new(Mutex::new(CacheBudgetState {
                pressure_level: Some(CachePressureLevel::None),
                ..Default::default()
            })),
        })
    }

    pub fn from_mebibytes(hard_limit_mib: u64) -> Option<Self> {
        hard_limit_mib
            .checked_mul(1024 * 1024)
            .and_then(|bytes| Self::new(bytes, 0.8, 0.65))
    }

    pub fn observe(&self, resident_bytes: u64) -> CachePressureAction {
        let mut state = self.lock();
        let previous = state.pressure_level.unwrap_or(CachePressureLevel::None);
        let level = if resident_bytes >= self.hard_limit_bytes {
            CachePressureLevel::Hard
        } else if resident_bytes >= self.soft_limit_bytes {
            CachePressureLevel::Soft
        } else if resident_bytes <= self.target_bytes {
            CachePressureLevel::None
        } else {
            previous
        };
        if level != CachePressureLevel::None && level != previous {
            state.pressure_events = state.pressure_events.saturating_add(1);
        }
        state.pressure_level = Some(level);
        CachePressureAction {
            level,
            target_bytes: self.target_bytes,
            to_free_bytes: resident_bytes.saturating_sub(self.target_bytes),
            stop_speculation: level == CachePressureLevel::Hard,
        }
    }

    pub fn record_eviction(&self, kind: impl Into<String>, entries: u64) {
        let mut state = self.lock();
        let entry = state.evictions.entry(kind.into()).or_default();
        *entry = entry.saturating_add(entries);
    }

    pub fn snapshot(&self, resident_bytes: u64) -> CacheBudgetSnapshot {
        let state = self.lock();
        CacheBudgetSnapshot {
            hard_limit_bytes: self.hard_limit_bytes,
            soft_limit_bytes: self.soft_limit_bytes,
            target_bytes: self.target_bytes,
            resident_bytes,
            pressure_level: state.pressure_level.unwrap_or(CachePressureLevel::None),
            pressure_events: state.pressure_events,
            evictions: state.evictions.clone(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, CacheBudgetState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Fixture {
        contract: String,
        schema_version: u32,
        hard_limit_bytes: u64,
        soft_ratio: f64,
        target_ratio: f64,
        observations: Vec<Observation>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Observation {
        resident_bytes: u64,
        level: CachePressureLevel,
        to_free_bytes: u64,
        stop_speculation: bool,
    }

    #[test]
    fn replays_cross_runtime_pressure_contract() {
        let fixture: Fixture = serde_json::from_str(include_str!(
            "../../../tests/fixtures/cache-budget-contract.json"
        ))
        .unwrap();
        assert_eq!(fixture.contract, "ruvyxa.cache-budget");
        assert_eq!(fixture.schema_version, 1);
        let budget = CacheBudget::new(
            fixture.hard_limit_bytes,
            fixture.soft_ratio,
            fixture.target_ratio,
        )
        .unwrap();

        for observation in fixture.observations {
            let action = budget.observe(observation.resident_bytes);
            assert_eq!(action.level, observation.level);
            assert_eq!(action.to_free_bytes, observation.to_free_bytes);
            assert_eq!(action.stop_speculation, observation.stop_speculation);
        }
    }

    #[test]
    fn rejects_invalid_or_overflowing_budgets() {
        assert!(CacheBudget::new(0, 0.8, 0.65).is_none());
        assert!(CacheBudget::new(100, 0.5, 0.6).is_none());
        assert!(CacheBudget::from_mebibytes(u64::MAX).is_none());
    }
}
