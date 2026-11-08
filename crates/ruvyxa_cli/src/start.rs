//! `ruvyxa start`: run a build on the standalone server.
//!
//! Production has one request host — `createHandler` inside the standalone
//! server every self-hosted adapter emits. `start` used to serve the build
//! through the Axum host instead, which made it a second production host that
//! had to agree with the first through a table of conformance fixtures. It is a
//! supervisor now: it finds the standalone server the build left, checks it
//! belongs to this build, and runs it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use ruvyxa_dev_server::{JavaScriptRuntime, serve};
use ruvyxa_diagnostics::{Diagnostic, RuvyxaError};

use crate::runtime_config::{
    ensure_build_output_exists, load_project_config, manifest_build_id, production_server_config,
};
use crate::{ProjectConfig, StartArgs, warn_text};

/// The file a staged standalone server records its build id in.
pub(crate) const STANDALONE_BUILD_ID_FILE: &str = ".ruvyxa-build-id";

/// Where the standalone server for `runtime` lives in the build output: the
/// directory the built-in adapter of the same name writes.
pub(crate) fn standalone_dir(out_dir: &Path, runtime: JavaScriptRuntime) -> PathBuf {
    out_dir.join("deploy").join(runtime.command())
}

/// The built-in adapter to run for `start`, or `None` when the build's own
/// adapter stage already produced that directory.
///
/// An official adapter named by its package (`@ruvyxa/adapter-node`) is the
/// same adapter as its short name: the runner resolves `node` to that package,
/// so comparing the spellings would run it twice into one directory.
pub(crate) fn local_standalone_adapter(
    runtime: JavaScriptRuntime,
    selected: Option<&str>,
) -> Option<&'static str> {
    let selected = selected.map(|name| name.strip_prefix("@ruvyxa/adapter-").unwrap_or(name));
    (selected != Some(runtime.command())).then_some(runtime.command())
}

/// Settle the local standalone stage's result into the build's.
///
/// `local_adapter` is what [`local_standalone_adapter`] chose: `None` when the
/// build's own adapter output is the standalone server, so failing to stamp it
/// is a failure of the build the project asked for. `Some` when an extra pass
/// was run only so `ruvyxa start` has something to run: the project asked for
/// another deployment (vercel, netlify, ...) and that one already succeeded, so
/// a failure here must not take it down. It becomes a warning, the stamp is
/// skipped, and `ruvyxa start` reports `RUV1022` if it is ever run.
///
/// `Ok(Some(warning))` is the warning to print.
pub(crate) fn settle_local_standalone_stage(
    runtime: JavaScriptRuntime,
    local_adapter: Option<&str>,
    outcome: anyhow::Result<()>,
) -> anyhow::Result<Option<String>> {
    match (local_adapter, outcome) {
        (_, Ok(())) => Ok(None),
        (None, Err(error)) => Err(error),
        (Some(adapter), Err(error)) => Ok(Some(format!(
            "could not stage the local {adapter} standalone server for `ruvyxa start` ({} runtime); the deployment output is unaffected, and `ruvyxa start` will report RUV1022 until a build stages it: {error:#}",
            runtime.command()
        ))),
    }
}

/// Record which build the staged server belongs to.
pub(crate) fn stamp_standalone(out_dir: &Path, runtime: JavaScriptRuntime) -> anyhow::Result<()> {
    let build_id = manifest_build_id(out_dir)
        .context("the build output has no deploy.buildId to stamp the standalone server with")?;
    let stamp = standalone_dir(out_dir, runtime).join(STANDALONE_BUILD_ID_FILE);
    ruvyxa_bundler::atomic_file::write_atomic(&stamp, build_id.as_bytes())
        .with_context(|| format!("failed to write {}", stamp.display()))
}

pub(crate) enum StandaloneState {
    Current,
    Missing,
    Stale,
}

pub(crate) fn standalone_state(out_dir: &Path, runtime: JavaScriptRuntime) -> StandaloneState {
    let dir = standalone_dir(out_dir, runtime);
    if !dir.join("server").join("index.mjs").is_file() {
        return StandaloneState::Missing;
    }
    let stamped = std::fs::read_to_string(dir.join(STANDALONE_BUILD_ID_FILE))
        .ok()
        .map(|value| value.trim().to_string());
    match (stamped, manifest_build_id(out_dir)) {
        (Some(stamped), Some(current)) if stamped == current => StandaloneState::Current,
        _ => StandaloneState::Stale,
    }
}

pub(crate) struct LaunchPlan {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) env: BTreeMap<String, String>,
}

/// Arguments before the entry point. Deno's are the ones the deno smoke lane
/// runs a deployment with; `script_args` carries `--node-modules-dir=manual`,
/// which is for tool processes inside a project, not for a self-contained
/// deployment directory.
fn entry_args(runtime: JavaScriptRuntime) -> &'static [&'static str] {
    match runtime {
        JavaScriptRuntime::Node | JavaScriptRuntime::Bun => &[],
        JavaScriptRuntime::Deno => &["run", "-A", "--no-prompt"],
    }
}

pub(crate) fn launch_plan(
    runtime: JavaScriptRuntime,
    standalone: &Path,
    host: &str,
    port: u16,
    workers: Option<&str>,
    image_service: Option<(&str, &str)>,
    project_env: BTreeMap<String, String>,
) -> LaunchPlan {
    let mut args: Vec<String> = entry_args(runtime)
        .iter()
        .map(|arg| (*arg).to_string())
        .collect();
    args.push("server/index.mjs".to_string());
    let mut env = project_env;
    env.insert("HOST".to_string(), host.to_string());
    env.insert("PORT".to_string(), port.to_string());
    if let Some(workers) = workers {
        env.insert("RUVYXA_WORKERS".to_string(), workers.to_string());
    }
    if let Some((url, token)) = image_service {
        env.insert("RUVYXA_IMAGE_SERVICE".to_string(), url.to_string());
        env.insert("RUVYXA_IMAGE_SERVICE_TOKEN".to_string(), token.to_string());
    }
    LaunchPlan {
        program: runtime.executable(),
        args,
        cwd: standalone.to_path_buf(),
        env,
    }
}

pub(crate) fn standalone_not_current(
    out_dir: &Path,
    runtime: JavaScriptRuntime,
    missing: bool,
) -> Diagnostic {
    let dir = standalone_dir(out_dir, runtime);
    // A server staged for another runtime is the likelier story when this one
    // is absent: the build ran under a different `--runtime` than the start.
    let staged_for: Vec<&'static str> = if missing {
        [
            JavaScriptRuntime::Node,
            JavaScriptRuntime::Bun,
            JavaScriptRuntime::Deno,
        ]
        .into_iter()
        .filter(|other| *other != runtime)
        .filter(|other| {
            standalone_dir(out_dir, *other)
                .join("server")
                .join("index.mjs")
                .is_file()
        })
        .map(JavaScriptRuntime::command)
        .collect()
    } else {
        Vec::new()
    };
    let explain = if missing {
        let mut explain = format!(
            "`ruvyxa start` runs the standalone server the build leaves at `{}`, and it is not there.",
            dir.display()
        );
        if !staged_for.is_empty() {
            explain.push_str(&format!(
                " The build output holds one for `{}` instead.",
                staged_for.join("`, `")
            ));
        }
        explain
    } else if !dir.join(STANDALONE_BUILD_ID_FILE).is_file() {
        format!(
            "The standalone server at `{}` carries no `{STANDALONE_BUILD_ID_FILE}` stamp, so nothing says it belongs to the build in `{}`.",
            dir.display(),
            out_dir.display()
        )
    } else {
        format!(
            "The standalone server at `{}` was staged by a different build than the one in `{}`, so it would serve old code.",
            dir.display(),
            out_dir.display()
        )
    };
    let suggestion = if staged_for.is_empty() {
        "Run `ruvyxa build` again, then `ruvyxa start`.".to_string()
    } else {
        format!(
            "Run `ruvyxa start --runtime {}` to serve the server that was staged, or `ruvyxa build` again for `{}`, then `ruvyxa start`.",
            staged_for[0],
            runtime.command()
        )
    };
    Diagnostic::new(
        "RUV1022",
        "Standalone server is missing or older than the build output",
    )
    .explain(explain)
    .at_file(&dir)
    .suggest(suggestion)
}

pub(crate) fn runtime_missing(runtime: JavaScriptRuntime) -> Diagnostic {
    Diagnostic::new(
        "RUV1023",
        "The JavaScript runtime `ruvyxa start` needs was not found",
    )
    .explain(format!(
        "The build is configured for `{}`, and `{}` could not be executed.",
        runtime.command(),
        runtime.executable().display()
    ))
    .suggest(format!(
        "Install {0}, or put it on PATH, or start with `--runtime` naming one that is installed and rebuild for it.",
        runtime.command()
    ))
}

/// `RUV1024` when the project configures a socket only the Axum host serves:
/// `realtime` (`/__ruvyxa/realtime`) or `collab` (presence, `/__ruvyxa/collab`).
/// `None` when it configures neither, so `start` can run the standalone server.
pub(crate) fn sockets_need_legacy_host(realtime: bool, collab: bool) -> Option<Diagnostic> {
    let configured = match (realtime, collab) {
        (false, false) => return None,
        (true, false) => "realtime (`/__ruvyxa/realtime`)",
        (false, true) => "collaboration presence (`/__ruvyxa/collab`)",
        (true, true) => {
            "realtime (`/__ruvyxa/realtime`) and collaboration presence (`/__ruvyxa/collab`)"
        }
    };
    Some(
        Diagnostic::new(
            "RUV1024",
            "Realtime and collaboration are served only by the legacy host",
        )
        .explain(format!(
            "`ruvyxa.config.ts` configures {configured}, whose WebSocket transport runs on the Axum host. `ruvyxa start` now runs the standalone server, which does not carry it."
        ))
        .suggest("Run `ruvyxa start --legacy-host` for now. Both transports are experimental."),
    )
}

/// The one conversion every refusal below goes through, as
/// `ensure_build_output_exists` does it.
fn refuse(diagnostic: Diagnostic) -> anyhow::Error {
    RuvyxaError::from(diagnostic).into()
}

/// The shared secret between this process and its child, from the OS-seeded
/// keys `RandomState` draws on each construction. Unpredictable to other local
/// processes, not cryptographic; the image service listens on loopback only,
/// so that is the threat it answers. No dependency is added for a stronger one.
fn image_service_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut token = String::with_capacity(32);
    for _ in 0..2 {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default(),
        );
        token.push_str(&format!("{:016x}", hasher.finish()));
    }
    token
}

/// `command` is the subcommand that was typed (`start` or `preview`), so the
/// deprecation warning names what the operator actually ran.
pub(crate) async fn start(args: StartArgs, command: &str) -> anyhow::Result<()> {
    let config: ProjectConfig = load_project_config(&args.server.root)?;
    ensure_build_output_exists(&args.server, &config)?;

    if args.legacy_host {
        eprintln!(
            "{}",
            warn_text(format!(
                "`ruvyxa {command} --legacy-host` serves through the previous host and will be removed in the next minor release."
            ))
        );
        return serve(production_server_config(&args.server, &config)?)
            .await
            .context("production server failed");
    }

    if let Some(diagnostic) =
        sockets_need_legacy_host(config.realtime.is_some(), config.collab.is_some())
    {
        return Err(refuse(diagnostic));
    }

    let server = production_server_config(&args.server, &config)?;
    let runtime = server.runtime;
    let out_dir = args.server.root.join(config.out_dir());
    match standalone_state(&out_dir, runtime) {
        StandaloneState::Current => {}
        StandaloneState::Missing => {
            return Err(refuse(standalone_not_current(&out_dir, runtime, true)));
        }
        StandaloneState::Stale => {
            return Err(refuse(standalone_not_current(&out_dir, runtime, false)));
        }
    }
    if !runtime.is_available() {
        return Err(refuse(runtime_missing(runtime)));
    }

    let token = image_service_token();
    // Held, not used, for the child's lifetime. Dropping a JoinHandle detaches
    // the task rather than aborting it, so this binding is not what keeps the
    // service alive; it names the task and keeps it reachable until
    // `std::process::exit` below ends both.
    let (image_url, _image_task) = if server.dynamic_images.enabled {
        let (address, task) = ruvyxa_dev_server::image_service::spawn_image_service(
            server.public_dir.clone(),
            server.dynamic_images.clone(),
            token.clone(),
        )
        .await
        .context("failed to start the image service")?;
        (Some(format!("http://{address}")), Some(task))
    } else {
        (None, None)
    };

    // The bind address `production_server_config` already resolved, once:
    // flags, then PORT/HOST, then config, then the production default.
    let plan = launch_plan(
        runtime,
        &standalone_dir(&out_dir, runtime),
        &server.host,
        server.port,
        args.workers.as_deref(),
        image_url.as_deref().map(|url| (url, token.as_str())),
        ruvyxa_dev_server::project_env(&args.server.root)?,
    );
    let code = supervise(plan).await?;
    std::process::exit(code);
}

/// Run the child to completion, forwarding termination, and return its code.
///
/// On unix the child gets its own process group, so a terminal's Ctrl-C reaches
/// only this process and is forwarded once. A hangup — the terminal or SSH
/// session closing — is forwarded as SIGTERM: left to its default action it
/// would end this process alone and orphan the child, which sits in its own
/// group and would keep holding the port. On Windows the console delivers
/// Ctrl-C to both processes and there is nothing to forward.
///
/// Some deliveries reach the child twice anyway: systemd's default
/// `KillMode=control-group` signals every process in the unit, so the child
/// gets SIGTERM directly and again from here. That is safe because the
/// standalone server reads a repeat signal inside its duplicate window as the
/// same stop and keeps draining; only a repeat after it forces an exit.
///
/// The signal streams are registered before the spawn, so a signal landing
/// between the two is queued for the loop instead of taking this process
/// down by its default action with the child already running.
async fn supervise(plan: LaunchPlan) -> anyhow::Result<i32> {
    #[cfg(unix)]
    let (mut terminate, mut interrupt, mut hangup) = {
        use tokio::signal::unix::{SignalKind, signal};
        (
            signal(SignalKind::terminate())?,
            signal(SignalKind::interrupt())?,
            signal(SignalKind::hangup())?,
        )
    };

    let mut command = tokio::process::Command::new(&plan.program);
    command
        .args(&plan.args)
        .current_dir(&plan.cwd)
        .envs(&plan.env)
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(target_os = "linux")]
    die_with_parent(&mut command);
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to run {}", plan.program.display()))?;
    // Held, not used, for as long as this function runs: closing it is what
    // ends the child, and the operating system closes it however this process
    // dies.
    #[cfg(windows)]
    let _tree = child
        .raw_handle()
        .and_then(|handle| KillOnClose::adopt(handle).ok());

    #[cfg(unix)]
    {
        loop {
            tokio::select! {
                status = child.wait() => return Ok(exit_code(status?)),
                _ = terminate.recv() => forward(&child, libc::SIGTERM),
                _ = interrupt.recv() => forward(&child, libc::SIGINT),
                _ = hangup.recv() => forward(&child, libc::SIGTERM),
            }
        }
    }
    #[cfg(not(unix))]
    {
        loop {
            tokio::select! {
                status = child.wait() => return Ok(exit_code(status?)),
                _ = tokio::signal::ctrl_c() => {}
            }
        }
    }
}

// `kill_on_drop` covers every way out of `supervise` that unwinds, and none
// that does not: a SIGKILL, an out-of-memory kill, `Stop-Process`, Task
// Manager, or a CI step timing out ends this process without running a single
// destructor. The standalone server then kept running, holding the port, and
// the next `ruvyxa start` failed on a port conflict with nothing of its own
// left to stop. The in-process host it replaces had no child to leave behind.
// So the operating system is asked to tie the child's life to this one.
// macOS has no equivalent short of the child watching its parent; there the
// forwarded signals and `kill_on_drop` are the whole of it.

/// Linux: the kernel sends the child SIGTERM when its parent thread exits.
///
/// "Parent thread", not process: the death signal follows the thread that
/// forked. That is safe here because the spawn runs on the async task's own
/// thread, a runtime worker, and the runtime lives as long as the process. A
/// spawn moved onto the blocking pool would break that, since its idle threads
/// exit.
#[cfg(target_os = "linux")]
fn die_with_parent(command: &mut tokio::process::Command) {
    let parent = std::process::id();
    // SAFETY: the closure runs in the forked child before exec and calls only
    // prctl(2) and getppid(2), both async-signal-safe; it allocates nothing
    // and touches no lock.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // The parent may have died between fork and prctl, in which case
            // no signal is coming: the child has already been reparented.
            if u32::try_from(libc::getppid()).ok() != Some(parent) {
                return Err(std::io::Error::other("the supervisor exited during launch"));
            }
            Ok(())
        });
    }
}

/// Windows: a job object that ends every process in it when its last handle
/// closes, and the child put in it.
///
/// Processes the child starts afterwards (cluster workers) join the job too, so
/// the whole tree goes. Nested jobs are supported since Windows 8, so a
/// supervisor that is itself in a job (a CI runner's, a terminal's) still works.
#[cfg(windows)]
struct KillOnClose(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl KillOnClose {
    fn adopt(process: std::os::windows::io::RawHandle) -> std::io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        // SAFETY: an unnamed job with default security; a null return is the
        // only failure and is checked.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let job = Self(job);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let size = u32::try_from(std::mem::size_of_val(&limits)).unwrap_or(u32::MAX);
        // SAFETY: `limits` is a live, correctly sized
        // JOBOBJECT_EXTENDED_LIMIT_INFORMATION for the class named, and `job.0`
        // is the handle just created.
        let configured = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                size,
            )
        };
        if configured == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: both handles are open: the job above, and the child's process
        // handle, which tokio holds until the child is reaped.
        if unsafe { AssignProcessToJobObject(job.0, process.cast()) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(job)
    }
}

#[cfg(windows)]
impl Drop for KillOnClose {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `adopt` and is closed exactly once.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(unix)]
fn forward(child: &tokio::process::Child, signal: libc::c_int) {
    let Some(pid) = child.id().and_then(|pid| libc::pid_t::try_from(pid).ok()) else {
        return;
    };
    // SAFETY: kill(2) on a pid this process spawned and is still waiting on, so
    // it cannot have been reused; no memory is read or written.
    unsafe {
        libc::kill(pid, signal);
    }
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    status.code().unwrap_or(1)
}

#[cfg(all(test, windows))]
mod kill_on_close_tests {
    use super::KillOnClose;
    use std::os::windows::io::AsRawHandle;
    use std::time::{Duration, Instant};

    /// The job is what outlives a supervisor that died without unwinding: the
    /// operating system closes its handle, and the child goes with it.
    #[test]
    fn closing_the_job_ends_the_child() {
        let node = ruvyxa_dev_server::JavaScriptRuntime::Node;
        if !node.is_available() {
            eprintln!("skipping: node is not available on this machine");
            return;
        }
        let mut child = std::process::Command::new(node.executable())
            .args(["-e", "setInterval(() => {}, 1000)"])
            .spawn()
            .unwrap();
        let job = KillOnClose::adopt(child.as_raw_handle()).unwrap();
        assert!(
            child.try_wait().unwrap().is_none(),
            "the child must be running"
        );

        drop(job);

        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("the child outlived its job");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
