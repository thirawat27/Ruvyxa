//! The block every command opens with, and the line a successful one closes on.
//!
//! Both used to be written twice. `ruvyxa_cli::ui` had a `print_tui_header`,
//! and the dev server printed its own copy of the same four lines inline —
//! which is why the dev server was for a long time the one surface with no
//! command badge, and why fixing that meant editing two files that had no idea
//! about each other. The header is drawn here now and nowhere else; the CLI and
//! the server each pass a title.
//!
//! The header is deliberately four lines and not a box. A framed banner has to
//! choose a width, and it is then either wider than the fields underneath it or
//! narrower than the tables — Ruvyxa prints both. A wordmark, a rule that stops
//! where every other rule stops, and a badge line need no width at all, and
//! they leave the header aligned with the fields it introduces.

use std::path::Path;
use std::time::Duration;

use crate::gradient::BRAND;
use crate::layout::{
    RULE_END_COLUMN, current_timestamp, format_duration, path_text, print_field, rule_line,
};
use crate::mascot::{MASCOT, badge, glyphs, wordmark};
use crate::theme::{brand, capabilities, dim};

/// The rule under a header runs from the text indent to the column every other
/// rule ends at, so a header and the section titles below it share one edge.
const HEADER_RULE_WIDTH: usize = RULE_END_COLUMN - 2;

/// Prints the standard command header: wordmark, rule, badge, and the run's
/// timestamp.
///
/// `title` is the command's own name only — `Build`, `Doctor`,
/// `Dev Server` — and the product half is added here so no caller can spell it
/// differently.
pub fn print_header(title: impl AsRef<str>) {
    let title = title.as_ref();
    println!();
    println!("  {MASCOT} {}", BRAND.paint(wordmark(title)));
    println!("  {}", rule_line(HEADER_RULE_WIDTH));
    println!("{}", badge_line(title));
    println!();
    print_field("time", dim(current_timestamp()));
}

/// The icon and tagline line, composed but not written.
///
/// Two columns are left after the icon rather than one. Three of the badge code
/// points default to text presentation and carry U+FE0F to ask for emoji
/// presentation instead (see [`crate::mascot::Badge`]); a terminal that honours
/// that request by drawing the glyph two columns wide while still advancing the
/// cursor by one will bleed it over whatever comes next, which is how the
/// balance scale ended up welded to the front of its own tagline. Nothing in a
/// terminal reports the mismatch, so the only fix available is to not be
/// standing there — and one spare column costs nothing where the advance is
/// already correct.
pub fn badge_line(title: &str) -> String {
    let badge = badge(title);
    format!("  {}  {}", badge.icon, dim(badge.tagline))
}

/// The line a command ends on when it succeeded. The mascot appears here and
/// nowhere else in a result, so a finished run is recognisable by shape from
/// across the room.
pub fn print_success_banner(message: impl AsRef<str>, duration: Duration) {
    print_success_banner_at(message, None, duration);
}

/// The same banner with a location. The message and the path are painted
/// separately — a path is the one part of the line a reader copies, and it
/// carries the same blue it has in every field above.
pub fn print_success_banner_at(message: impl AsRef<str>, path: Option<&Path>, duration: Duration) {
    let location = match path {
        Some(path) => format!(" {}", path_text(path)),
        None => String::new(),
    };
    println!(
        "\n  {} {} {}{} {}\n",
        // The mascot is product identity, which is what the `brand` role is
        // for. Painting it with the wordmark ramp instead would spend a
        // gradient on a glyph that renders in its own colours anyway.
        brand(MASCOT),
        BRAND.paint(glyphs(capabilities()).sparkle),
        BRAND.paint(message.as_ref()),
        location,
        // Deliberately not the phase line's ageing counter: a build that took
        // twenty seconds succeeded, and colouring its total as a warning would
        // make every honest result look like a complaint.
        dim(format!("· {}", format_duration(duration)))
    );
}
