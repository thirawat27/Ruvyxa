//! Server port binding with sequential fallback and conflict diagnostics.

use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::process::Command;

use ruvyxa_diagnostics::{Diagnostic, Result, RuvyxaError};
use tokio::net::TcpListener;

use crate::ServerConfig;
use crate::cli_output::{accent, dim, warn_text};

/// Highest port offset tried past the requested port before giving up.
pub(crate) const PORT_FALLBACK_SCAN_LIMIT: u16 = 100;

/// The furthest port past the requested one this server may move to.
///
/// Scanning forward is a convenience for a developer reading the terminal, and
/// `config.watch` is what separates that reader from a production process: it
/// is set by `ServerConfig::dev` and cleared by `ServerConfig::production`,
/// which `ruvyxa start` and `ruvyxa preview` both use. A container routes to
/// the port it was configured with, so a `ruvyxa start` that quietly took the
/// next one is healthy-looking and unreachable — the health check fails, the
/// supervisor restarts it, and the real cause (usually the previous instance
/// still holding the port) is reported only as a line on stdout. The other
/// self-hosted host, the generated standalone server, has always read `PORT`
/// and let `EADDRINUSE` surface as a crash the supervisor can act on.
pub(crate) fn port_fallback_scan_limit(config: &ServerConfig) -> u16 {
    if config.watch {
        PORT_FALLBACK_SCAN_LIMIT
    } else {
        0
    }
}

/// Bind every address the configured host answers to, on one shared port.
///
/// A host is not an address. `localhost` is two of them on any dual-stack
/// machine, and serving only the one the resolver returned first is why
/// `http://127.0.0.1:3000` answered "connection refused" from a server that was
/// happily serving `[::1]:3000` — the exact shape `proxy_pass
/// http://127.0.0.1:3000`, a container health probe, and most CI scripts are
/// written in.
///
/// Binding all of them also *is* the port-availability check. Taking one family
/// while another process holds the other succeeds on every platform, and then
/// whichever family the client's resolver picks decides whose server it
/// reaches; that is how two projects "shared" port 3000 without either
/// reporting a conflict. Holding the whole set leaves no window between
/// checking and binding.
pub(crate) async fn bind_listeners(
    config: &ServerConfig,
    address: SocketAddr,
) -> Result<(Vec<TcpListener>, SocketAddr)> {
    let mut first_addr_in_use = None;
    let targets = bind_addresses(&config.host, address.ip());

    for offset in 0..=port_fallback_scan_limit(config) {
        let Some(port) = address.port().checked_add(offset) else {
            break;
        };

        match bind_every(&targets, port).await {
            Ok(listeners) => {
                let bound_address = listeners
                    .first()
                    .and_then(|listener| listener.local_addr().ok())
                    .unwrap_or_else(|| SocketAddr::new(address.ip(), port));
                if offset > 0 {
                    print_port_fallback(config, address, bound_address);
                }
                return Ok((listeners, bound_address));
            }
            Err(BindFailure::PortUnavailable(error)) => {
                if offset == 0 {
                    first_addr_in_use = Some(error);
                }
            }
            Err(BindFailure::Fatal { target, source }) => {
                return Err(RuvyxaError::Io {
                    message: format!(
                        "Failed to bind server address {}",
                        SocketAddr::new(target, port)
                    ),
                    source,
                });
            }
        }
    }

    let error =
        first_addr_in_use.unwrap_or_else(|| std::io::Error::from(ErrorKind::AddrNotAvailable));
    Err(port_conflict_diagnostic(config, address, &error).into())
}

/// Why one port could not be taken across the whole address set.
enum BindFailure {
    /// Something already holds the port on at least one address. The next port
    /// is worth trying.
    PortUnavailable(std::io::Error),
    /// The primary address cannot be bound for a reason that changing port will
    /// not fix — an interface that does not exist, for instance.
    Fatal {
        target: IpAddr,
        source: std::io::Error,
    },
}

/// Take `port` on every address, or leave it free on all of them.
///
/// Partial success is the thing to avoid: a half-bound port would serve some
/// clients and refuse others. Dropping the listeners collected so far releases
/// what was taken, so the caller can move to the next port cleanly.
async fn bind_every(
    targets: &[IpAddr],
    port: u16,
) -> std::result::Result<Vec<TcpListener>, BindFailure> {
    let mut listeners = Vec::with_capacity(targets.len());
    // Port 0 asks the OS to choose, and it chooses per socket. Every address
    // has to end up on the *same* port or the host is answering on two of them,
    // so the first assignment becomes the port the rest are held to.
    let mut port = port;

    for (index, target) in targets.iter().enumerate() {
        match TcpListener::bind(SocketAddr::new(*target, port)).await {
            Ok(listener) => {
                if port == 0 {
                    port = listener.local_addr().map(|bound| bound.port()).unwrap_or(0);
                }
                listeners.push(listener);
            }
            // AddrInUse: another process owns the port. PermissionDenied:
            // Windows returns WSAEACCES (10013) for ports inside an excluded
            // port range (Hyper-V/WinNAT reservations); both mean "this port is
            // unavailable, try the next one" rather than a fatal failure.
            Err(error)
                if error.kind() == ErrorKind::AddrInUse
                    || error.kind() == ErrorKind::PermissionDenied =>
            {
                return Err(BindFailure::PortUnavailable(error));
            }
            // Only the primary address has to be bindable. A secondary that
            // cannot be bound at all — the IPv6 loopback on a host without
            // IPv6 — is not a conflict and not a failure: it is simply not an
            // address this machine answers on.
            Err(source) if index == 0 => {
                return Err(BindFailure::Fatal {
                    target: *target,
                    source,
                });
            }
            Err(_) => {}
        }
    }

    if listeners.is_empty() {
        return Err(BindFailure::PortUnavailable(std::io::Error::from(
            ErrorKind::AddrNotAvailable,
        )));
    }
    Ok(listeners)
}

/// Every address the configured host answers to, the resolved one first.
///
/// Order matters only for which address is reported as bound and which one's
/// failure is fatal; the set is what decides reachability.
fn bind_addresses(host: &str, primary: IpAddr) -> Vec<IpAddr> {
    let mut addresses = vec![primary];
    addresses.extend(
        format!("{host}:0")
            .to_socket_addrs()
            .map(|resolved| resolved.map(|address| address.ip()).collect::<Vec<_>>())
            .unwrap_or_default(),
    );

    // Loopback is one destination with two addresses. A resolver that answers
    // with only one family still leaves the other reachable as `localhost`, so
    // serve both whenever either appears. An explicit `0.0.0.0` or a real
    // interface address is left exactly as asked for.
    if addresses.iter().any(IpAddr::is_loopback) {
        addresses.push(IpAddr::V4(Ipv4Addr::LOCALHOST));
        addresses.push(IpAddr::V6(Ipv6Addr::LOCALHOST));
    }

    // Order-preserving, so the primary stays first.
    let mut seen = std::collections::HashSet::new();
    addresses.retain(|address| seen.insert(*address));
    addresses
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
    let scan = port_fallback_scan_limit(config);
    let os_hint = port_lookup_hint(address.port());

    // A production host scans nothing, so the message must not describe a
    // range it never tried — and it does not need to: the owning PID above is
    // the whole answer, which makes the production message the better of the
    // two. The title stays the one RUV1201 has always carried, because
    // `one_diagnostic_code_carries_one_meaning` reads these literals and a
    // second title here would be a new collision, not a clearer message.
    if scan == 0 {
        return Diagnostic::new("RUV1201", "No available server port was found")
            .explain(format!(
                "{}:{} could not be bound ({error}).{owner}\n\nA production server binds the port it was given rather than moving to another one, because a container, proxy, or health check routes to the configured port and would not follow.",
                config.host,
                address.port(),
            ))
            .suggest(format!(
                "Stop the process using port {}, or set `PORT` / pass `--port <free-port>`. {os_hint}",
                address.port(),
            ));
    }

    let end_port = address.port().saturating_add(scan);
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
    // Bounded like every other child process Ruvyxa starts. This runs only
    // after a port conflict, to name the process holding the port; a probe that
    // hangs would replace a clear error with no error at all.
    let mut command = Command::new("netstat");
    command.args(["-ano", "-p", "tcp"]);
    let output =
        crate::process::output_with_timeout(&mut command, crate::process::PROBE_TIMEOUT).ok()?;
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

    let mut tasklist = Command::new("tasklist");
    tasklist.args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"]);
    let process = crate::process::output_with_timeout(&mut tasklist, crate::process::PROBE_TIMEOUT)
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
    // `lsof` is the likeliest of these to stall — it walks every open file on
    // the machine and blocks on unresponsive mounts — so the bound matters most
    // here.
    let mut command = Command::new("lsof");
    command.args(["-nP", "-iTCP", "-sTCP:LISTEN"]);
    let output =
        crate::process::output_with_timeout(&mut command, crate::process::PROBE_TIMEOUT).ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let suffix = format!(":{port}");
    stdout.lines().skip(1).find_map(|line| {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        // `lsof` puts the listening address last (`*:3000`, `127.0.0.1:3000`).
        // A `contains` test would match the wrong row, because `:300` is a
        // substring of `:3000`; anchor on the address column's end instead.
        let address = columns.last()?;
        if !address.ends_with(&suffix) {
            return None;
        }
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
