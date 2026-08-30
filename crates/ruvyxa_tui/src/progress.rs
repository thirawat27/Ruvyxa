//! Live progress: the runner track and the phase spinner.
//!
//! Both repaint the current line with a carriage return, so both are gated on
//! [`Capabilities::animate`] — a pipe, a log file, `TERM=dumb`, or `RUVYXA_FUN=0`
//! gets no repainting at all, and the phase line printed at the end is the only
//! record of the work. That is the same rule the old TTY-only progress bar
//! followed; it is stated once here instead of at each call site.
//!
//! Every transient frame is written to **stderr**; every line that survives the
//! run — the phase line, fields, tables, banners — stays on stdout. The split
//! is what makes the spinner safe: it ticks from its own thread while a phase
//! blocks, and a phase body that prints to stdout (a user's TypeScript plugin
//! calling `console.log` from a `resolve` or `transform` hook, for instance)
//! now lands on a different stream instead of tearing the spinner's line in
//! half. It also means `ruvyxa build > log` records results without animation
//! bytes, which is the convention progress reporting already follows.
//!
//! Two frames still must not interleave with each other, so a phase that can
//! report progress uses [`ProgressTrack`] — driven from the working thread —
//! rather than starting a second spinner.
//!
//! # Why the track owns its clock
//!
//! [`ProgressTrack`] is a value rather than a pair of free functions because a
//! remaining-time estimate needs a start instant, and the alternative was
//! threading one through every call site or keeping a process-global one that
//! two concurrent phases would share. Owning the clock also means the line is
//! cleared on drop, so a phase that fails half way through does not print its
//! error over a half-drawn track.
//!
//! # Clearing
//!
//! Both erase with `\x1b[2K` rather than by printing spaces. A fixed run of
//! spaces has to guess the line's width: guess low and the tail of the previous
//! frame survives, guess high and every clear wraps on a terminal narrower than
//! the guess, leaving a blank line behind. `animate` already proves a real
//! terminal, and every real terminal implements erase-in-line.

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::gradient::{PULSE, TRAIL};
use crate::layout::{PHASE_LABEL_WIDTH, display_width, format_duration, spaces};
use crate::mascot::{Glyphs, glyphs};
use crate::theme::{
    Capabilities, ColorDepth, alert_text, capabilities, dim, label, number, ok_text, warn_text,
};

/// Cells of track, excluding the runner itself. The runner is drawn between the
/// two halves, so the rendered width is constant as it advances.
///
/// Sized so the whole line — glyph, label, track, percentage, count, and
/// estimate — fits inside eighty columns. The runner is an emoji and occupies
/// two of them.
pub const TRACK_WIDTH: usize = 22;

const SPINNER_INTERVAL: Duration = Duration::from_millis(90);

/// How far the shimmer travels along a ramp per repaint. Slow enough to read as
/// a highlight moving through the glyph rather than as a colour flicker.
const SHIMMER_STEP: f64 = 0.055;

/// A phase whose elapsed time has passed one of these is worth noticing, so the
/// counter changes role rather than staying uniformly dim. Nothing is wrong at
/// either threshold — the point is that a build stuck on one phase looks
/// different from a build working through several.
const SLOW_PHASE: Duration = Duration::from_secs(5);
const VERY_SLOW_PHASE: Duration = Duration::from_secs(20);

/// Advanced on every repaint so the dust behind the runner alternates and the
/// shimmer travels even while the underlying count is unchanged.
static TICK: AtomicUsize = AtomicUsize::new(0);

const ERASE_LINE: &str = "\r\x1b[2K";

fn clear_line() {
    eprint!("{ERASE_LINE}");
    let _ = std::io::stderr().flush();
}

/// A unit of work being counted down, drawn as a fox running a track.
///
/// Silent unless the terminal accepts animation, so piped output and CI logs
/// stay one line per event.
pub struct ProgressTrack {
    name: String,
    total: usize,
    started: Instant,
    enabled: bool,
}

impl ProgressTrack {
    /// Begins the track and draws its first frame. `enabled` is the caller's
    /// own decision — a build that is not printing a summary passes `false` —
    /// and is checked in addition to the terminal's answer, never instead of
    /// it.
    pub fn start(enabled: bool, name: &str, total: usize) -> Self {
        let track = Self {
            name: name.to_string(),
            total,
            started: Instant::now(),
            enabled: enabled && total > 0 && capabilities().animate,
        };
        track.set(0);
        track
    }

    /// Repaints the track at `done` of the total it was started with.
    pub fn set(&self, done: usize) {
        if !self.enabled {
            return;
        }

        let tick = TICK.fetch_add(1, Ordering::Relaxed);
        eprint!(
            "{ERASE_LINE}{}",
            track_line(
                capabilities(),
                &self.name,
                done,
                self.total,
                tick,
                self.remaining(done)
            )
        );
        let _ = std::io::stderr().flush();
    }

    /// A time estimate, or nothing at all until one job has finished — an
    /// estimate extrapolated from zero samples is not a slower estimate, it is
    /// a made-up one.
    fn remaining(&self, done: usize) -> Option<Duration> {
        if done == 0 || done >= self.total {
            return None;
        }
        let per_job = self.started.elapsed().as_secs_f64() / done as f64;
        Some(Duration::from_secs_f64(
            per_job * (self.total - done) as f64,
        ))
    }
}

/// One frame of the track, composed but not written.
///
/// Pure, and public for the same reason [`runner_cells`] is: a frame is drawn
/// only when both streams are terminals, so a test process can never see one.
/// Everything that can be wrong about the line — its width on an eighty-column
/// terminal, whether the percentage agrees with the count, whether the estimate
/// appears before there is anything to estimate from — is decided here, where
/// it can be asserted.
pub fn track_line(
    capabilities: Capabilities,
    name: &str,
    done: usize,
    total: usize,
    tick: usize,
    remaining: Option<Duration>,
) -> String {
    let estimate = match remaining {
        Some(remaining) => format!(" · ~{}", format_duration(remaining)),
        None => String::new(),
    };
    format!(
        "  {} {}{} {} {} {}{}",
        PULSE.paint_cycled_with(
            capabilities.depth,
            glyphs(capabilities).pending,
            tick as f64 * SHIMMER_STEP * 2.0
        ),
        label(name),
        spaces(PHASE_LABEL_WIDTH, display_width(name)),
        runner_track(capabilities, done, total, tick),
        number(format!("{:>3}%", percent(done, total))),
        // The count is padded to the total's own width. Left as it comes, the
        // line grew a column the moment `9/30` became `10/30`, which pushed
        // everything after it one cell right for the rest of the phase.
        dim(format!(
            "{done:>width$}/{total}",
            width = decimal_width(total)
        )),
        dim(estimate),
    )
}

fn decimal_width(value: usize) -> usize {
    value.to_string().len()
}

impl Drop for ProgressTrack {
    /// Leaves the line clear whether the phase finished or unwound, so whatever
    /// prints next starts at the left edge.
    fn drop(&mut self) {
        if self.enabled {
            clear_line();
        }
    }
}

fn percent(done: usize, total: usize) -> usize {
    if total == 0 {
        return 0;
    }
    (done.min(total) * 100) / total
}

/// The coloured track: the green ground the runner has covered, a puff of dust
/// under its feet, and the ground still ahead of it.
///
/// Only the covered half carries a ramp. The half ahead stays in the `dim` role
/// it has always been — it is the background the bar is measured against, and a
/// second colour there competes with the one that means something.
///
/// The covered half is walked cell by cell rather than across a string, because
/// its colour has to mean *distance behind the runner* and the runner moves.
pub fn runner_track(capabilities: Capabilities, done: usize, total: usize, tick: usize) -> String {
    let glyphs = glyphs(capabilities);
    let (behind, runner, ahead) = runner_cells(glyphs, done, total, tick);
    format!(
        "{}{}{}",
        trail(&behind, capabilities.depth),
        runner,
        dim(ahead)
    )
}

/// Paints the covered ground across the whole of [`TRAIL`], so the first cell
/// lands on its first stop and the last on its last however many cells there
/// are — the ramp stretches with the bar rather than running out part way.
fn trail(behind: &str, depth: ColorDepth) -> String {
    let total = behind.chars().count();
    if total == 0 {
        return String::new();
    }
    let span = (total.saturating_sub(1)).max(1) as f64;
    behind
        .chars()
        .enumerate()
        .map(|(index, cell)| TRAIL.cell(depth, &cell.to_string(), index as f64 / span))
        .collect()
}

/// The unstyled halves of the track. Split out from [`runner_track`] because
/// the position and dust arithmetic is what can be wrong, and it is testable
/// only without escape codes in the way.
pub fn runner_cells(
    glyphs: Glyphs,
    done: usize,
    total: usize,
    tick: usize,
) -> (String, &'static str, String) {
    let filled = (TRACK_WIDTH * done.min(total))
        .checked_div(total)
        .unwrap_or(0);

    let mut behind = glyphs.filled.repeat(filled);
    if filled > 0 {
        // The cell the runner just left shows dust instead of solid ground.
        behind.truncate(behind.len() - glyphs.filled.len());
        behind.push_str(glyphs.dust[tick % glyphs.dust.len()]);
    }

    (
        behind,
        glyphs.runner,
        glyphs.empty.repeat(TRACK_WIDTH - filled),
    )
}

/// A phase line: what ran, what it produced, how long it took.
pub fn print_phase(name: &str, detail: String, duration: Duration) {
    crate::stream::print_line(&phase_line(name, detail, duration));
}

pub fn phase_line(name: &str, detail: String, duration: Duration) -> String {
    format!(
        "  {} {}{} {} {}",
        ok_text(glyphs(capabilities()).done),
        label(name),
        spaces(PHASE_LABEL_WIDTH, display_width(name)),
        crate::theme::accent(detail),
        elapsed_text(duration)
    )
}

/// The trailing `· 1.20s` on a phase or spinner line, coloured by how long the
/// phase has been running.
fn elapsed_text(duration: Duration) -> String {
    let text = format!("· {}", format_duration(duration));
    if duration >= VERY_SLOW_PHASE {
        alert_text(text)
    } else if duration >= SLOW_PHASE {
        warn_text(text)
    } else {
        dim(text)
    }
}

/// A phase that has started but cannot report progress: the label spins with a
/// live elapsed counter until [`Spinner::finish`] replaces it with the result.
///
/// When the terminal does not accept animation nothing is drawn while the phase
/// runs, and `finish` prints the same phase line the non-animated build always
/// printed.
pub struct Spinner {
    started: Instant,
    name: String,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Spinner {
    pub fn start(name: &str) -> Self {
        let started = Instant::now();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = if capabilities().animate {
            let stop = Arc::clone(&stop);
            let name = name.to_string();
            Some(std::thread::spawn(move || {
                spin(&stop, &name, started);
            }))
        } else {
            None
        };

        Self {
            started,
            name: name.to_string(),
            stop,
            handle,
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Stops the animation and prints the phase result, timed from `start`.
    pub fn finish(self, detail: String) {
        let elapsed = self.started.elapsed();
        self.finish_with(detail, elapsed);
    }

    /// Stops the animation and prints the phase result with a caller-supplied
    /// duration, for a phase that measures only part of its own span.
    pub fn finish_with(mut self, detail: String, duration: Duration) {
        self.stop_animation();
        print_phase(&self.name, detail, duration);
    }

    /// Stops the animation without printing, for a phase that turned out to
    /// have nothing to report.
    pub fn cancel(mut self) {
        self.stop_animation();
    }

    fn stop_animation(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
            clear_line();
        }
    }
}

/// The spinner's own thread. The glyph shimmers along [`PULSE`] while the
/// counter ages through its roles, so a phase that is merely slow and a phase
/// that has hung look different without either needing a message.
fn spin(stop: &AtomicBool, name: &str, started: Instant) {
    let capabilities = capabilities();
    let mut frame = 0usize;
    while !stop.load(Ordering::Relaxed) {
        eprint!(
            "{ERASE_LINE}{}",
            spinner_line(capabilities, name, frame, started.elapsed())
        );
        let _ = std::io::stderr().flush();
        frame += 1;
        std::thread::sleep(SPINNER_INTERVAL);
    }
}

/// One frame of the spinner, composed but not written. Pure for the same
/// reason [`track_line`] is.
pub fn spinner_line(
    capabilities: Capabilities,
    name: &str,
    frame: usize,
    elapsed: Duration,
) -> String {
    let frames = glyphs(capabilities).spinner;
    format!(
        "  {} {}{} {}",
        PULSE.paint_cycled_with(
            capabilities.depth,
            frames[frame % frames.len()],
            frame as f64 * SHIMMER_STEP
        ),
        label(name),
        spaces(PHASE_LABEL_WIDTH, display_width(name)),
        elapsed_text(elapsed)
    )
}

impl Drop for Spinner {
    fn drop(&mut self) {
        // A phase that fails unwinds past `finish`; the thread must still stop,
        // or the error message is printed over a spinning line.
        self.stop_animation();
    }
}
