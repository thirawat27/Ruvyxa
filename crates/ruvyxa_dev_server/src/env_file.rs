//! `.env` / `.env.local` loading for project config and JavaScript runtimes.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use ruvyxa_diagnostics::{Result, RuvyxaError};

/// Loads `.env` and `.env.local` from the project root, later files winning.
pub fn project_env(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();

    for file_name in [".env", ".env.local"] {
        let file = root.join(file_name);
        if !file.exists() {
            continue;
        }

        let source = fs::read_to_string(&file).map_err(|source| RuvyxaError::Io {
            message: format!("Failed to read {}", file.display()),
            source,
        })?;

        for (key, value) in parse_env_source(&source) {
            values.insert(key, value);
        }
    }

    Ok(values)
}

pub(crate) fn parse_env_source(source: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();

    for line in source.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        let key = key.trim();
        if key.is_empty() {
            continue;
        }

        values.insert(key.to_string(), unquote_env_value(value.trim()));
    }

    values
}

fn unquote_env_value(value: &str) -> String {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}
