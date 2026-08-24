//! The Ruvyxa fox, and every other glyph the command line draws with.
//!
//! The fox already existed in the command headers as a static emoji. It moves
//! now — it runs the length of a progress track, the same character the demo's
//! `ruvyxa-runner` mini-game puts on screen — but only where movement is safe:
//! a real terminal that has not opted out of animation.
//!
//! Two glyph sets exist, and [`glyphs`] is the only thing that picks between
//! them. That matters more than it looks: a hand-written `if unicode` at a call
//! site is how a table ends up drawn with box characters and closed with ASCII
//! ones on the same terminal.
//!
//! [`tui_header_title`] deliberately does *not* consult terminal capabilities.
//! The header emoji is part of the product name in every transcript, including
//! piped output, and a test pins that spelling.

use crate::theme::Capabilities;

/// The nine corners and joins of a table border, plus its two rules.
///
/// Kept together rather than as nine fields on [`Glyphs`] because they are only
/// ever correct as a set — a rounded top with a square bottom is worse than
/// either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub top_left: &'static str,
    pub top_join: &'static str,
    pub top_right: &'static str,
    pub mid_left: &'static str,
    pub mid_join: &'static str,
    pub mid_right: &'static str,
    pub bottom_left: &'static str,
    pub bottom_join: &'static str,
    pub bottom_right: &'static str,
    pub horizontal: &'static str,
    pub vertical: &'static str,
}

/// Glyphs for one drawing style. Two sets exist so a terminal without box
/// drawing still gets a readable track rather than replacement characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyphs {
    pub filled: &'static str,
    pub empty: &'static str,
    /// Two frames of dust kicked up behind the runner. Alternating these is
    /// what makes the fox look like it is running rather than sliding.
    pub dust: [&'static str; 2],
    pub runner: &'static str,
    pub spinner: &'static [&'static str],
    pub done: &'static str,
    pub failed: &'static str,
    pub pending: &'static str,
    /// Drawn once when a run finishes, next to the mascot.
    pub sparkle: &'static str,
    /// The rule under a header and after a section title.
    pub rule: &'static str,
    /// The upright tick that marks a section title, so a group heading is
    /// findable in a screen of fields without reading any of them.
    pub marker: &'static str,
    /// The cell a magnitude bar is built from.
    pub bar: &'static str,
    pub frame: Frame,
}

pub const UNICODE_GLYPHS: Glyphs = Glyphs {
    filled: "▰",
    empty: "▱",
    dust: ["·", "˙"],
    runner: "🦊",
    spinner: &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
    done: "✓",
    failed: "✗",
    pending: "◌",
    sparkle: "✦",
    rule: "─",
    marker: "▍",
    bar: "▇",
    frame: Frame {
        top_left: "╭",
        top_join: "┬",
        top_right: "╮",
        mid_left: "├",
        mid_join: "┼",
        mid_right: "┤",
        bottom_left: "╰",
        bottom_join: "┴",
        bottom_right: "╯",
        horizontal: "─",
        vertical: "│",
    },
};

pub const ASCII_GLYPHS: Glyphs = Glyphs {
    filled: "#",
    empty: "-",
    dust: [".", ","],
    runner: ">",
    spinner: &["|", "/", "-", "\\"],
    done: "+",
    failed: "x",
    pending: "o",
    sparkle: "*",
    rule: "-",
    marker: "|",
    bar: "=",
    frame: Frame {
        top_left: "+",
        top_join: "+",
        top_right: "+",
        mid_left: "+",
        mid_join: "+",
        mid_right: "+",
        bottom_left: "+",
        bottom_join: "+",
        bottom_right: "+",
        horizontal: "-",
        vertical: "|",
    },
};

pub fn glyphs(capabilities: Capabilities) -> Glyphs {
    if capabilities.unicode {
        UNICODE_GLYPHS
    } else {
        ASCII_GLYPHS
    }
}

/// The fox itself. One constant, because the header, the success banner, and
/// the running track all draw it and a second spelling would be invisible
/// until somebody compared two transcripts.
pub const MASCOT: &str = "🦊";

/// The product half of a header title, without the mascot: `Ruvyxa Build`.
///
/// Split out because the header paints this and leaves the mascot alone — an
/// emoji renders in its own colours and a gradient stop spent on it is a stop
/// the wordmark does not get.
pub fn wordmark(title: impl AsRef<str>) -> String {
    format!("Ruvyxa {}", title.as_ref())
}

/// The title used by every command header. Stable across terminals by design.
pub fn tui_header_title(title: impl AsRef<str>) -> String {
    format!("{MASCOT} {}", wordmark(title))
}

/// The icon and one-line tagline under a command's title.
///
/// The fox stays on the title line so every command still announces the same
/// product; the badge is what makes `doctor` recognisable from `clean` at a
/// glance in a scrollback full of runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Badge {
    pub icon: &'static str,
    pub tagline: &'static str,
}

/// Matched on the title's first word, so `Benchmark (3 sample(s))` resolves the
/// same as `Benchmark`. This table is the only place a command's identity is
/// decided; a command missing from it falls back to the mascot.
pub(crate) const BADGES: [(&str, Badge); 12] = [
    (
        "Dev",
        Badge {
            icon: "⚡",
            tagline: "hot reload · route watching · HMR",
        },
    ),
    (
        "Server",
        Badge {
            icon: "🚀",
            tagline: "serving the production build",
        },
    ),
    (
        "Build",
        Badge {
            icon: "📦",
            tagline: "compile · bundle · prerender · ship",
        },
    ),
    (
        "Routes",
        Badge {
            icon: "🧭",
            tagline: "every path this app answers",
        },
    ),
    (
        "Analyze",
        Badge {
            icon: "🔍",
            tagline: "routes · imports · server/client boundaries",
        },
    ),
    (
        "Check",
        Badge {
            icon: "🧪",
            tagline: "production readiness, end to end",
        },
    ),
    (
        "Doctor",
        Badge {
            icon: "🩺",
            tagline: "versions · project · toolchain · adapter · graph",
        },
    ),
    (
        "Clean",
        Badge {
            icon: "🧹",
            tagline: "remove generated output",
        },
    ),
    (
        "Parity",
        Badge {
            icon: "⚖️",
            tagline: "dev and prod must agree",
        },
    ),
    (
        "Benchmark",
        Badge {
            icon: "⏱️",
            tagline: "config · routes · cold and warm builds · render",
        },
    ),
    (
        "Plugin",
        Badge {
            icon: "🧩",
            tagline: "a publishable extension package",
        },
    ),
    (
        "Adds",
        Badge {
            icon: "✨",
            tagline: "framework-native starting points",
        },
    ),
];

pub fn badge(title: &str) -> Badge {
    let first_word = title.split_whitespace().next().unwrap_or_default();
    BADGES
        .iter()
        .find(|(name, _)| *name == first_word)
        .map(|(_, badge)| *badge)
        .unwrap_or(Badge {
            icon: "🦊",
            tagline: "the Ruvyxa framework",
        })
}
