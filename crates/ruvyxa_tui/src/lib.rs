//! Terminal presentation for every Ruvyxa binary.
//!
//! Colour, field layout, tables, progress, and the mascot lived in two places
//! before this crate existed — `ruvyxa_cli::ui` and
//! `ruvyxa_dev_server::cli_output` — with the same ANSI codes, the same
//! capability checks, and two different label widths, which is why `ruvyxa dev`
//! and `ruvyxa build` printed their values in different columns. Both now
//! re-export from here, so a change to how Ruvyxa looks is a change in one
//! file.
//!
//! This crate is a leaf: it depends on nothing in the workspace, and nothing
//! here knows what a route, a bundle, or a request is. It decides how output
//! looks and never what output means.
//!
//! # The modules, and the line between two of them
//!
//! - [`theme`] — what the terminal can do, and the **roles**: the sixteen-colour
//!   palette that carries every distinction the output makes.
//! - [`gradient`] — **decoration**: 24-bit ramps that carry no distinction and
//!   therefore may be as rich as the terminal allows, each naming the single
//!   role it collapses to when it cannot.
//! - [`mascot`] — every glyph, in a Unicode set and an ASCII one.
//! - [`layout`] — fields, sections, rules, tables, and unit formatting.
//! - [`progress`] — the two live surfaces: the runner track and the spinner.
//! - [`banner`] — the header every command opens with and the line a
//!   successful one closes on.
//! - [`sanitize`] — what a value is allowed to be once it is on its way to a
//!   terminal, since repository file paths reach one directly.
//! - [`stream`] — the one writer every durable line goes through, where a
//!   closed pipe is a clean stop rather than a panic.
//!
//! The split between `theme` and `gradient` is the one worth holding on to.
//! Adding a 24-bit colour to a role looks like an improvement and is a
//! regression: a terminal that cannot render it approximates, and two roles
//! that meant different things become one colour on somebody else's machine. A
//! ramp has nothing to lose that way, which is why it lives somewhere else.

pub mod banner;
pub mod gradient;
pub mod layout;
pub mod mascot;
pub mod progress;
pub mod sanitize;
pub mod stream;
pub mod theme;

pub use banner::*;
pub use gradient::{BRAND, Gradient, HEAT, PULSE, RULE, Rgb, TRAIL, to_ansi256};
pub use layout::*;
pub use mascot::*;
pub use progress::*;
pub use sanitize::*;
pub use stream::*;
pub use theme::*;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_visual;
