import type { StandaloneServerRuntime } from './standalone-server.js'

/**
 * How many request-serving processes `RUVYXA_WORKERS` asks for.
 *
 * Emitted into the generated server with `Function.prototype.toString`, so the
 * function these tests call and the one a deployment runs are the same text.
 * It must therefore stay self-contained: no imports, no closures.
 */
export function resolveWorkerCount(value: string | undefined, parallelism: number): number {
  const raw = (value ?? '').trim().toLowerCase()
  if (raw === '') return 1
  if (raw === 'auto') return Math.max(1, Math.floor(parallelism))
  if (!/^[1-9][0-9]*$/.test(raw)) {
    throw new Error(
      'RUV1025 RUVYXA_WORKERS must be a positive integer or "auto", got "' + String(value) + '"',
    )
  }
  return Number(raw)
}

/**
 * Whether a worker has crashed often enough that respawning is hiding a fault.
 *
 * Self-contained for the same reason as `resolveWorkerCount`. More than
 * `limit` crashes inside `windowMs` makes the supervisor exit non-zero, so the
 * orchestrator in front of it sees a failing process instead of a quiet loop.
 */
export function crashLoopExceeded(
  crashTimes: readonly number[],
  now: number,
  limit = 5,
  windowMs = 60_000,
): boolean {
  return crashTimes.filter((time) => now - time < windowMs).length > limit
}

/**
 * The crash times still inside the crash-loop window, oldest first.
 *
 * Self-contained for the same reason as `resolveWorkerCount`. The supervisor
 * keeps only these, so its record does not grow for the life of the process
 * and the respawn delay is derived from recent crashes alone: a worker that
 * crashed a few times last week does not wait out the cap today.
 */
export function recentCrashes(
  crashTimes: readonly number[],
  now: number,
  windowMs = 60_000,
): number[] {
  return crashTimes.filter((time) => now - time < windowMs)
}

/**
 * How long to wait before respawning, given the crashes inside the window
 * (this one included): doubling from half a second, capped at 30 seconds.
 *
 * Self-contained for the same reason as `resolveWorkerCount`.
 */
export function respawnDelay(recentCrashCount: number): number {
  return Math.min(30_000, 250 * 2 ** Math.min(Math.max(recentCrashCount, 1), 7))
}

/**
 * The supervisor half of the generated server.
 *
 * Node uses `node:cluster`, which shares the listening socket on every
 * platform and gives each worker an IPC channel — the channel is how a drain
 * is requested, because on Windows a signal sent to a worker kills it outright.
 * Bun has no cluster primitive, so it spawns processes that listen with
 * `reusePort`, which the kernel only load-balances on Linux; elsewhere more than
 * one worker is refused rather than approximated. Deno has neither, and refuses.
 */
export function clusterSource(runtime: StandaloneServerRuntime): string {
  const rules = `${resolveWorkerCount.toString()}

${crashLoopExceeded.toString()}

${recentCrashes.toString()}

${respawnDelay.toString()}

let WORKER_COUNT = 1;
try {
  WORKER_COUNT = resolveWorkerCount(process.env.RUVYXA_WORKERS, os.availableParallelism());
} catch (error) {
  log('error', error instanceof Error ? error.message : String(error));
  process.exit(1);
}
`
  if (runtime === 'deno') {
    return `${rules}
if (WORKER_COUNT > 1) {
  log('error', 'RUV1025 RUVYXA_WORKERS above 1 is not supported on Deno; run one process per instance');
  process.exit(1);
}
const IS_SUPERVISOR = false;
function runSupervisor() {}
`
  }

  const spawnWorker =
    runtime === 'node'
      ? `function spawnWorker() {
  return cluster.fork();
}`
      : `function spawnWorker() {
  // With an IPC channel, so a drain is asked for by message exactly as on Node,
  // and so a worker learns its supervisor is gone when the channel closes.
  return spawn(process.execPath, [...process.execArgv, ...process.argv.slice(1)], {
    env: { ...process.env, RUVYXA_WORKERS: '1', RUVYXA_REUSE_PORT: '1' },
    stdio: ['inherit', 'inherit', 'inherit', 'ipc'],
  });
}`

  const stopWorkers = `${spawnWorker}
function stopWorker(worker) {
  // A message, not a signal: on Windows a signal sent to a worker ends it
  // outright, and a signal that also reached the worker from its process group
  // would be a second one, which is the forced exit. The drain is the point.
  try {
    worker.send({ ruvyxa: 'shutdown' });
  } catch {
    killWorker(worker, 'SIGTERM');
  }
}
function killWorker(worker, signal) {
  // A cluster worker carries its child process; a spawned one is it.
  try {
    (worker.process ?? worker).kill(signal);
  } catch {
    // already gone
  }
}`

  const imports =
    runtime === 'node'
      ? "import cluster from 'node:cluster';"
      : "import { spawn } from 'node:child_process';"

  const supervisorAllowed =
    runtime === 'node' ? 'cluster.isPrimary' : "process.env.RUVYXA_REUSE_PORT !== '1'"

  const platformGuard =
    runtime === 'bun'
      ? `  if (process.platform !== 'linux') {
    log('error', 'RUV1025 RUVYXA_WORKERS above 1 needs SO_REUSEPORT load balancing, which Bun gets only on Linux');
    process.exit(1);
  }
`
      : ''

  // Bun can only use more than one worker where the kernel balances
  // `reusePort`, so the hint is not offered where following it would fail.
  const hintAllowed = runtime === 'bun' ? "process.platform === 'linux'" : 'true'

  return `${imports}

${rules}
const IS_SUPERVISOR = WORKER_COUNT > 1 && ${supervisorAllowed};

// One line, once, for a server left at its default of one process on a machine
// with more to give. Only when nothing was asked for: an explicit
// RUVYXA_WORKERS=1 is a decision, and every worker of a cluster inherits a set
// RUVYXA_WORKERS, so the hint is never repeated per worker.
if (
  WORKER_COUNT === 1 &&
  (process.env.RUVYXA_WORKERS ?? '').trim() === '' &&
  os.availableParallelism() > 1 &&
  ${hintAllowed}
) {
  log('info', 'serving with one process; set RUVYXA_WORKERS (ruvyxa start --workers) to run more', {
    available_parallelism: os.availableParallelism(),
  });
}

${stopWorkers}

function runSupervisor() {
${platformGuard}  let crashes = [];
  let live = 0;
  let stopping = false;
  // The first worker that did not drain cleanly decides the supervisor's own
  // exit code, so an orchestrator sees a shutdown that went wrong as one.
  let worstExit = 0;
  const workers = new Set();

  function exitAfterKilling(signal, code) {
    for (const worker of workers) killWorker(worker, signal);
    process.exit(code);
  }

  function start() {
    // A respawn scheduled before a shutdown began must not start a worker after it.
    if (stopping) return;
    const worker = spawnWorker();
    live += 1;
    workers.add(worker);
    worker.on('exit', (code, signal) => {
      live -= 1;
      workers.delete(worker);
      if (stopping) {
        let status = typeof code === 'number' ? code : 0;
        if (status === 0 && signal) status = 1;
        if (worstExit === 0 && status !== 0) worstExit = status;
        if (live === 0) process.exit(worstExit);
        return;
      }
      const now = Date.now();
      crashes = recentCrashes([...crashes, now], now);
      if (crashLoopExceeded(crashes, now)) {
        log('error', 'worker crash loop, exiting', { crashes: crashes.length });
        // The healthy ones are stopped, not drained: this process exits right
        // after signalling them, which closes their IPC channels, and a worker
        // exits when its channel disconnects. None is left running with no
        // supervisor to ever stop it.
        exitAfterKilling('SIGTERM', 1);
      }
      const delay = respawnDelay(crashes.length);
      log('warn', 'worker exited, respawning', { code, signal, delay_ms: delay });
      setTimeout(start, delay);
    });
  }

  for (let index = 0; index < WORKER_COUNT; index += 1) start();

  function stop(reason) {
    if (stopping) {
      log('warn', 'shutdown forced', { reason });
      exitAfterKilling('SIGKILL', 1);
    }
    stopping = true;
    log('info', 'stopping workers', { reason, workers: live });
    // Every worker may already be gone, waiting out a respawn delay: no exit
    // event is coming to end the process, so it ends here.
    if (live === 0) process.exit(worstExit);
    for (const worker of workers) stopWorker(worker);
    const forceExit = setTimeout(() => {
      log('error', 'workers did not drain in time', { workers: live });
      exitAfterKilling('SIGKILL', 1);
    }, SHUTDOWN_GRACE_MS + DRAIN_DELAY_MS);
    if (typeof forceExit?.unref === 'function') forceExit.unref();
  }
  // The shared registration, so a repeat inside the duplicate window is
  // ignored here exactly as in a single process: a cgroup-wide SIGTERM plus a
  // forwarded one must not SIGKILL workers that are draining.
  onShutdownSignal(stop);
  log('info', 'supervising workers', { workers: WORKER_COUNT, runtime: RUVYXA_RUNTIME });
}
`
}
