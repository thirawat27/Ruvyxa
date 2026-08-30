//! Terminal capability detection and the single colour palette.
//!
//! Every colour in the Ruvyxa command line resolves to a role named here —
//! `accent`, `ok`, `warn`, `alert`, `link`, `label`, `dim`, `brand`, and the
//! heading colour that only [`HEADING_CODE`] still names.
//! Call sites ask for a role, never for an ANSI code, which is what stops two
//! commands from picking different greens for the same idea.
//!
//! Capability detection runs exactly once per process ([`capabilities`]).
//! [`paint`] used to re-check `is_terminal()` and read two environment
//! variables on every coloured fragment; that was invisible while output was
//! static, but an animated line repaints many fragments per frame, so the
//! answer is cached in a `OnceLock` instead.
//!
//! Two streams are asked, because the CLI writes to two: results go to stdout
//! and decide `color`, transient animation goes to stderr and decides
//! `animate`. Animation needs *both* to be terminals — the frames carry the
//! colours resolved for stdout, and a run whose results are being captured is a
//! run that should report one line per event on either stream.
//!
//! # Two colour systems, one meaning
//!
//! Roles stay inside the 16-colour range. That is not conservatism for its own
//! sake: a role *carries meaning* — `ok` against `warn`, a page route against
//! an API route — and a terminal that cannot render a 24-bit code approximates
//! it, which is how two roles collapse into one indistinguishable colour on
//! somebody else's machine.
//!
//! Decoration is the other half, and it has no such duty. A gradient across a
//! wordmark, a comet trail behind the progress runner, a rule that fades out —
//! none of them are the only carrier of anything, so they are free to ask for
//! 24-bit colour where it exists and fall back to a single role code where it
//! does not. [`ColorDepth`] is what separates the two, and
//! [`crate::gradient`] is where the decorative half lives.
//!
//! # Opt-outs and opt-ins
//!
//! - stdout is not a terminal — no colour, no animation
//! - stderr is not a terminal — no animation
//! - `NO_COLOR` is set — no colour
//! - `TERM=dumb` — no colour, no animation
//! - `RUVYXA_FUN=0` (or `false`, `off`, `no`, empty) — no animation, no mascot
//! - `RUVYXA_ASCII=1` — ASCII glyphs only, for terminals without box drawing
//! - `FORCE_COLOR` / `CLICOLOR_FORCE` — colour even when stdout is redirected,
//!   which is what makes a CI log that renders ANSI look like the terminal it
//!   is imitating. `FORCE_COLOR=1|2|3` also pins the depth to 16, 256, or
//!   24-bit.
//!
//! `FORCE_COLOR` outranks both `NO_COLOR` and `TERM=dumb`, because it is the
//! only one of the three a user sets deliberately for *this* run — the other
//! two are usually ambient, inherited from a shell profile or a CI image.
//! Forcing colour never forces animation: frames are repainted with a carriage
//! return, and a log file has nowhere to repaint to.

use std::io::IsTerminal;
use std::sync::OnceLock;

/// How many colours the attached terminal can be asked for.
///
/// Ordered, so a call site can ask `depth >= ColorDepth::Ansi256` rather than
/// matching every variant it does not care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ColorDepth {
    /// No escapes at all.
    None,
    /// The eight base colours and their bright variants. Every *role* resolves
    /// here, on every terminal that has colour at all.
    Ansi16,
    /// The 6×6×6 cube plus the 24-step grey ramp.
    Ansi256,
    /// 24-bit `38;2;r;g;b`.
    TrueColor,
}

/// What the attached terminal can be asked to do. Resolved once per process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// ANSI colour escapes are safe to emit. Equivalent to
    /// `depth != ColorDepth::None`, kept as its own field because almost every
    /// call site asks the yes/no question and not the how-many one.
    pub color: bool,
    /// How rich the colour may be. Only decoration reads this; a role never
    /// does.
    pub depth: ColorDepth,
    /// Carriage-return repainting on stderr is safe: both streams are real
    /// terminals and the user has not opted out. Everything that redraws a line
    /// must check this, so piped output and CI logs keep one line per event.
    pub animate: bool,
    /// Non-ASCII glyphs (box drawing, braille, emoji) are safe to emit.
    pub unicode: bool,
}

impl Capabilities {
    /// Plain, unconditional output: what `TERM=dumb` gets. A pipe differs — it
    /// keeps `unicode`, because a log file renders UTF-8 whatever produced it.
    pub const PLAIN: Self = Self {
        color: false,
        depth: ColorDepth::None,
        animate: false,
        unicode: false,
    };
}

pub fn capabilities() -> Capabilities {
    static CAPABILITIES: OnceLock<Capabilities> = OnceLock::new();
    *CAPABILITIES.get_or_init(|| {
        detect_capabilities(
            std::io::stdout().is_terminal(),
            std::io::stderr().is_terminal(),
            |name| std::env::var_os(name).map(|value| value.to_string_lossy().into_owned()),
        )
    })
}

/// The depth every decorative gradient resolves against.
pub fn color_depth() -> ColorDepth {
    capabilities().depth
}

/// The detection rules, taking both terminal answers and the environment as
/// arguments so the decision table can be tested without a terminal.
pub fn detect_capabilities(
    stdout_is_terminal: bool,
    stderr_is_terminal: bool,
    env: impl Fn(&str) -> Option<String>,
) -> Capabilities {
    detect_capabilities_on(stdout_is_terminal, stderr_is_terminal, env, cfg!(windows))
}

/// The whole decision table, with the platform taken as an argument for the
/// same reason the two terminal answers are.
///
/// [`detect_depth`] already accepted `windows_console` so its table would stay
/// decidable in a test on either platform, and that only holds if the caller
/// passes it too — with `cfg!(windows)` baked in one function further up, every
/// assertion about the `TERM` and `COLORTERM` fallbacks passed on Linux and
/// failed on Windows, which is the platform the branch exists for.
pub fn detect_capabilities_on(
    stdout_is_terminal: bool,
    stderr_is_terminal: bool,
    env: impl Fn(&str) -> Option<String>,
    windows_console: bool,
) -> Capabilities {
    let dumb = env("TERM")
        .map(|term| term.eq_ignore_ascii_case("dumb"))
        .unwrap_or(false);
    let forced = forced_depth(&env);

    let depth = match forced {
        Some(depth) => depth,
        None if dumb || env("NO_COLOR").is_some() || !stdout_is_terminal => ColorDepth::None,
        None => detect_depth(&env, windows_console),
    };

    Capabilities {
        color: depth != ColorDepth::None,
        depth,
        animate: stdout_is_terminal
            && stderr_is_terminal
            && !dumb
            && !is_off(env("RUVYXA_FUN").as_deref()),
        // Not gated on either terminal answer: a redirected log renders UTF-8 as well as
        // a terminal does, and the header emoji is already written
        // unconditionally. Only a terminal that cannot draw the glyphs — or a
        // user who says so — falls back to ASCII.
        unicode: !dumb && !is_on(env("RUVYXA_ASCII").as_deref()),
    }
}

/// `FORCE_COLOR` / `CLICOLOR_FORCE`, read as an explicit answer rather than a
/// hint. The numeric levels follow the convention every other CLI already
/// uses, so a developer who exports `FORCE_COLOR=3` once gets the same result
/// from Ruvyxa as from the rest of their toolchain.
fn forced_depth(env: &impl Fn(&str) -> Option<String>) -> Option<ColorDepth> {
    let value = env("FORCE_COLOR").or_else(|| env("CLICOLOR_FORCE"))?;
    let value = value.trim();
    if is_off(Some(value)) {
        // `FORCE_COLOR=0` is the documented way to say "no colour", and it has
        // to win over the same variable's presence.
        return Some(ColorDepth::None);
    }
    Some(match value {
        "1" => ColorDepth::Ansi16,
        "2" => ColorDepth::Ansi256,
        // Anything else set and not negative means "yes, as much as you have".
        _ => ColorDepth::TrueColor,
    })
}

/// What the terminal advertises when nobody has forced an answer.
///
/// Every branch here is a claim the terminal makes about itself, so a wrong
/// guess costs an approximated gradient and never a lost distinction — roles do
/// not consult this.
///
/// `windows_console` is taken as an argument rather than read from `cfg!` here
/// so the whole table stays decidable in a test on either platform. It is the
/// branch that matters most in practice: a native PowerShell session sets
/// **none** of the variables above — no `COLORTERM`, no `TERM`, no
/// `TERM_PROGRAM`, and not even `WT_SESSION` unless the process was started by
/// Windows Terminal itself. Every Unix-world signal is absent, so this fell
/// through to `Ansi16` and collapsed every gradient in the product to a flat
/// fallback on the platform the framework is most often developed on. The
/// Windows console has rendered 24-bit colour since Windows 10 1703; a console
/// that draws our escapes at all draws those, and an older one approximates
/// rather than garbles.
fn detect_depth(env: &impl Fn(&str) -> Option<String>, windows_console: bool) -> ColorDepth {
    let colorterm = env("COLORTERM").unwrap_or_default();
    if colorterm.eq_ignore_ascii_case("truecolor") || colorterm.eq_ignore_ascii_case("24bit") {
        return ColorDepth::TrueColor;
    }

    // Windows Terminal sets no `COLORTERM` and reports `TERM` only under WSL,
    // so its own marker is the only signal available on a native shell — and it
    // has been 24-bit since it shipped.
    if env("WT_SESSION").is_some() {
        return ColorDepth::TrueColor;
    }

    let program = env("TERM_PROGRAM").unwrap_or_default();
    if TRUECOLOR_PROGRAMS
        .iter()
        .any(|known| program.eq_ignore_ascii_case(known))
    {
        return ColorDepth::TrueColor;
    }

    let term = env("TERM").unwrap_or_default().to_ascii_lowercase();
    if term.contains("truecolor") || term.contains("direct") {
        return ColorDepth::TrueColor;
    }

    // Above the `TERM` fallbacks on purpose. `TERM` on Windows is set by an
    // emulation layer — Git Bash exports `xterm` — and describes that layer
    // rather than the terminal actually drawing the pixels, so believing it
    // downgrades a 24-bit window because of what a shim inherited.
    if windows_console {
        return ColorDepth::TrueColor;
    }

    if term.contains("256") {
        return ColorDepth::Ansi256;
    }

    ColorDepth::Ansi16
}

/// Terminals that report 24-bit colour by name because they set no
/// `COLORTERM`. Matched whole, not by substring, so an unrelated program whose
/// name happens to contain one of these is not promoted by accident.
const TRUECOLOR_PROGRAMS: [&str; 5] = ["iTerm.app", "WezTerm", "ghostty", "vscode", "Hyper"];

/// A variable is "off" when it is set to an explicit negative. Following the
/// convention already used for adapter detection, an empty value counts as not
/// asking for the feature.
fn is_off(value: Option<&str>) -> bool {
    match value {
        None => false,
        Some(value) => {
            let value = value.trim();
            value.is_empty()
                || value == "0"
                || value.eq_ignore_ascii_case("false")
                || value.eq_ignore_ascii_case("off")
                || value.eq_ignore_ascii_case("no")
        }
    }
}

fn is_on(value: Option<&str>) -> bool {
    match value {
        None => false,
        Some(value) => !is_off(Some(value)),
    }
}

pub fn paint(value: impl AsRef<str>, code: &str) -> String {
    paint_when(capabilities().color, value, code)
}

/// The pure half of [`paint`]: colour decided by the caller.
///
/// The value is filtered through [`crate::sanitize::sanitize_plain`] first, and
/// on both branches. Repository file paths reach this function directly — the
/// route table prints one per route — and nothing this crate paints carries a
/// legitimate control character, so a name holding `\x1b[2J` or a run of
/// newlines and forged glyphs is a value rewriting the reader's screen. Doing
/// it here covers `path_text`, `label`, `dim`, `accent`, and every other role
/// at once.
///
/// Filtering when `color` is false is not belt and braces: colour is off in a
/// redirected CI log, which is the case the vector is aimed at.
pub fn paint_when(color: bool, value: impl AsRef<str>, code: &str) -> String {
    let value = crate::sanitize::sanitize_plain(value.as_ref());
    if !color {
        return value;
    }

    format!("\x1b[{code}m{value}\x1b[0m")
}

// ─── Roles ───────────────────────────────────────────────────────────────────
//
// A role says what a value *is*, and the palette decides what colour that
// becomes. Adding colour for decoration alone is what made every field cyan;
// the rule here is the one `styled_first_load` already follows — if two values
// carry different meaning, they get different colours, and if they carry the
// same meaning they get the same one everywhere.
//
// Every code stays inside the 16-colour range so a terminal without 256-colour
// support renders the same distinctions rather than approximating them.
// Decoration that carries no distinction is exempt and lives in
// `crate::gradient`.

/// The heading colour, as a code rather than as a role function.
///
/// It has no `heading(..)` any more because it has no direct caller: the only
/// heading Ruvyxa prints is the wordmark, and the wordmark is a gradient. What
/// the code still decides is how that gradient *degrades* — [`crate::gradient::BRAND`]
/// names this as its fallback, so a sixteen-colour terminal gets the bold
/// magenta header it always got. `ruvyxa --help` mirrors it a third time,
/// because clap takes colour values instead of escapes; a test pins the two
/// spellings together.
pub const HEADING_CODE: &str = "1;35";

/// The mascot and anything that carries the product's identity.
pub fn brand(value: impl AsRef<str>) -> String {
    paint(value, "1;33")
}

pub fn label(value: impl AsRef<str>) -> String {
    paint(value, "90")
}

/// A name, a word, a text value: the default for anything that is not a count,
/// a path, or a status.
pub fn accent(value: impl AsRef<str>) -> String {
    paint(value, "36")
}

/// A count or a measurement. Bright and bold so a number is findable in a
/// column of names — `doctor` prints twenty-five fields and the reader is
/// almost always looking for one of the eight numbers among them.
pub fn number(value: impl AsRef<str>) -> String {
    paint(value, "1;96")
}

/// Structural or descriptive information: a version, a target, a kind.
pub fn info(value: impl AsRef<str>) -> String {
    paint(value, "94")
}

/// A secondary classification that must stay distinguishable from [`info`] when
/// the two sit in the same column — page routes against API routes, for
/// instance.
pub fn note(value: impl AsRef<str>) -> String {
    paint(value, "95")
}

pub fn dim(value: impl AsRef<str>) -> String {
    paint(value, "90")
}

pub fn ok_text(value: impl AsRef<str>) -> String {
    paint(value, "32")
}

pub fn warn_text(value: impl AsRef<str>) -> String {
    paint(value, "33")
}

pub fn alert_text(value: impl AsRef<str>) -> String {
    paint(value, "31")
}

pub fn link(value: impl AsRef<str>) -> String {
    paint(value, "34")
}

/// The status glyph a passing step prints, drawn from the same glyph set as
/// the progress track so `check` and `build` mark success the same way.
pub fn success() -> String {
    ok_text(crate::mascot::glyphs(capabilities()).done)
}

pub fn error_label() -> String {
    alert_text(crate::mascot::glyphs(capabilities()).failed)
}
