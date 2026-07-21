//! Server port binding with sequential fallback and conflict diagnostics.

use std::io::ErrorKind;
use std::net::SocketAddr;
use std::process::Command;

use ruvyxa_diagnostics::{Diagnostic, Result, RuvyxaError};
use tokio::net::TcpListener;

use crate::ServerConfig;
use crate::cli_output::{accent, dim, warn_text};

/// Highest port offset tried past the requested port before giving up.
pub(crate) const PORT_FALLBACK_SCAN_LIMIT: u16 = 100;

pub(crate) async fn bind_listener(
    config: &ServerConfig,
    address: SocketAddr,
) -> Result<(TcpListener, SocketAddr)> {
    let mut first_addr_in_use = None;

    for offset in 0..=PORT_FALLBACK_SCAN_LIMIT {
        let Some(port) = address.port().checked_add(offset) else {
            break;
        };
        let mut candidate = address;
        candidate.set_port(port);

        let bind_result = TcpListener::bind(candidate).await;
        match bind_result {
            Ok(listener) => {
                let bound_address = listener.local_addr().unwrap_or(candidate);
                if offset > 0 {
                    print_port_fallback(config, address, bound_address);
                }
                return Ok((listener, bound_address));
            }
            // AddrInUse: another process owns the port. PermissionDenied:
            // Windows returns WSAEACCES (10013) for ports inside an excluded
            // port range (Hyper-V/WinNAT reservations); both mean "this port
            // is unavailable, try the next one" rather than a fatal failure.
            Err(error)
                if error.kind() == ErrorKind::AddrInUse
                    || error.kind() == ErrorKind::PermissionDenied =>
            {
                if offset == 0 {
                    first_addr_in_use = Some(error);
                }
            }
            Err(source) => {
                return Err(RuvyxaError::Io {
                    message: format!("Failed to bind server address {candidate}"),
                    source,
                });
            }
        }
    }

    let error =
        first_addr_in_use.unwrap_or_else(|| std::io::Error::from(ErrorKind::AddrNotAvailable));
    Err(port_conflict_diagnostic(config, address, &error).into())
}

fn print_port_fallback(config: &ServerConfig, requested: SocketAddr, bound: SocketAddr) {
    let message = format!(
        "Port {} is already in use; using {} instead.",
        requested.port(),
        bound.port()
    );
    tracing::warn!(
        requested = requested.port(),
        bound = bound.port(),
        "{message}"
    );
    println!("  {} {}", warn_text("warning"), accent(message));
    if let Some(owner) = port_owner(requested.port()) {
        println!("  {} {}", dim("port owner"), accent(owner));
    }
    println!(
        "  {} {}",
        dim("requested"),
        accent(format!("{}:{}", config.host, requested.port()))
    );
}

pub(crate) fn port_conflict_diagnostic(
    config: &ServerConfig,
    address: SocketAddr,
    error: &std::io::Error,
) -> Diagnostic {
    let owner = port_owner(address.port())
        .map(|owner| format!("\n\nDetected owner:\n  {owner}"))
        .unwrap_or_default();
    let end_port = address.port().saturating_add(PORT_FALLBACK_SCAN_LIMIT);
    let os_hint = port_lookup_hint(address.port());

    Diagnostic::new("RUV1201", "No available server port was found")
        .explain(format!(
            "{}:{} could not be bound, and Ruvyxa could not find a free port through {} ({error}).{owner}",
            config.host,
            address.port(),
            end_port
        ))
        .suggest(format!(
            "Stop the process using port {}, free a port in the {}-{} range, or pass `--port <free-port>`. {os_hint}",
            address.port(),
            address.port(),
            end_port
        ))
}

fn port_owner(port: u16) -> Option<String> {
    if cfg!(windows) {
        return windows_port_owner(port);
    }

    unix_port_owner(port)
}

fn windows_port_owner(port: u16) -> Option<String> {
    let output = Command::new("netstat")
        .args(["-ano", "-p", "tcp"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let pid = stdout.lines().find_map(|line| {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        let local = columns.get(1)?;
        let state = columns.get(3)?;
        let pid = columns.get(4)?;

        if local.ends_with(&format!(":{port}")) && state.eq_ignore_ascii_case("LISTENING") {
            Some((*pid).to_string())
        } else {
            None
        }
    })?;

    let process = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout
                .lines()
                .next()
                .and_then(|line| line.split(',').next())
                .map(|name| name.trim_matches('"').to_string())
        })
        .filter(|name| !name.is_empty());

    Some(match process {
        Some(process) => format!("PID {pid} ({process})"),
        None => format!("PID {pid}"),
    })
}

fn unix_port_owner(port: u16) -> Option<String> {
    let output = Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().skip(1).find_map(|line| {
        if !line.contains(&format!(":{port}")) {
            return None;
        }
        let columns = line.split_whitespace().collect::<Vec<_>>();
        let process = columns.first()?;
        let pid = columns.get(1)?;
        Some(format!("PID {pid} ({process})"))
    })
}

fn port_lookup_hint(port: u16) -> String {
    if cfg!(windows) {
        format!(
            "On Windows, inspect it with `Get-NetTCPConnection -LocalPort {port} | Select-Object OwningProcess`."
        )
    } else {
        format!("On macOS/Linux, inspect it with `lsof -nP -iTCP:{port} -sTCP:LISTEN`.")
    }
}
