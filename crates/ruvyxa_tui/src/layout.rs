//! Field, table, and unit formatting shared by every Ruvyxa command.
//!
//! The two label widths below are chosen together, not independently. A field
//! line is `"  " + label(22) + " "` and a phase line is `"  ✓ " + label(20) +
//! " "`, so both put their value in column 25 and the build summary reads as
//! one table instead of two. Changing one without the other reintroduces the
//! misalignment this module exists to remove. The width itself is set by the
//! longest label any command prints — `dependency duplicates`, from `doctor`.
//!
//! Rules — the line under a header, the line after a section title — all end at
//! the same column, [`RULE_END_COLUMN`], for the same reason: a screen of
//! `doctor` output has five of them, and five lengths chosen independently look
//! like five mistakes.
//!
//! Table borders come from [`Frame`] rather than from literals here, so the
//! rounded Unicode frame and the ASCII fallback cannot drift apart corner by
//! corner.

use std::path::Path;
use std::time::Duration;

use chrono::Local;

use crate::gradient::{HEAT, RULE};
use crate::mascot::{Frame, glyphs};
use crate::stream::{print_blank_line, print_fragment, print_line};
use crate::theme::{capabilities, color_depth, dim, label, ok_text, paint, warn_text};

/// Width of the label column in a `key: value` field line.
pub const FIELD_LABEL_WIDTH: usize = 22;

/// Width of the label column in a build-phase line, which carries a two-column
/// status glyph before the label.
pub const PHASE_LABEL_WIDTH: usize = 20;

/// The column every rule stops at, counted from the left edge of the line.
pub const RULE_END_COLUMN: usize = FIELD_LABEL_WIDTH + 8;

pub fn print_field(name: &str, value: String) {
    print_line(&field_line(name, value));
}

pub fn field_line(name: &str, value: String) -> String {
    format!(
        "  {}{} {}",
        label(name),
        // Padded by characters, not bytes. A label is almost always ASCII,
        // where the two agree - and that is exactly why `len()` survived here
        // until a label carrying a middle dot arrived one column short.
        spaces(FIELD_LABEL_WIDTH, display_width(name)),
        value
    )
}

/// Columns are measured in characters, not bytes. A byte count is the same
/// number for ASCII and three times too large for a box-drawing glyph, which is
/// what pushed the benchmark bar column out of its border.
pub fn display_width(value: &str) -> usize {
    value.chars().count()
}

/// Widths for a table whose columns are `headers` and whose cells are `rows`,
/// each column sized to its widest entry.
pub fn column_widths<const N: usize>(headers: &[&str; N], rows: &[[String; N]]) -> Vec<usize> {
    headers
        .iter()
        .enumerate()
        .map(|(index, header)| {
            rows.iter()
                .map(|row| display_width(&row[index]))
                .max()
                .unwrap_or(0)
                .max(display_width(header))
        })
        .collect()
}

pub fn spaces(width: usize, len: usize) -> String {
    " ".repeat(width.saturating_sub(len))
}

pub fn current_timestamp() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn format_duration(duration: Duration) -> String {
    if duration.as_secs() > 0 {
        format!("{:.2}s", duration.as_secs_f64())
    } else {
        format!("{:.0}ms", duration.as_secs_f64() * 1000.0)
    }
}

pub fn format_bytes(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;

    if bytes < KIB as usize {
        return format!("{bytes} B");
    }

    let kibibytes = bytes as f64 / KIB;
    if bytes < MIB as usize {
        return if kibibytes < 10.0 {
            format!("{kibibytes:.1} kB")
        } else {
            format!("{kibibytes:.0} kB")
        };
    }

    let mebibytes = bytes as f64 / MIB;
    if mebibytes < 10.0 {
        format!("{mebibytes:.1} MB")
    } else {
        format!("{mebibytes:.0} MB")
    }
}

/// Which of a table's three horizontal rules is being drawn.
///
/// The three used to be one function called three times, which is why every
/// table was closed with the same `+---+` it was opened with. A rounded frame
/// needs the caller to say which end it is at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableRule {
    Top,
    Middle,
    Bottom,
}

pub fn print_table_rule(widths: &[usize], rule: TableRule) {
    print_line(&table_rule_line(widths, rule));
}

pub fn table_rule_line(widths: &[usize], rule: TableRule) -> String {
    let frame = glyphs(capabilities()).frame;
    let (left, join, right) = match rule {
        TableRule::Top => (frame.top_left, frame.top_join, frame.top_right),
        TableRule::Middle => (frame.mid_left, frame.mid_join, frame.mid_right),
        TableRule::Bottom => (frame.bottom_left, frame.bottom_join, frame.bottom_right),
    };

    let mut line = String::from(left);
    for (index, width) in widths.iter().enumerate() {
        if index > 0 {
            line.push_str(join);
        }
        line.push_str(&frame.horizontal.repeat(width + 2));
    }
    line.push_str(right);
    format!("  {}", dim(line))
}

/// Prints one bordered table row. `right_aligned` decides each column
/// independently — numeric columns read right-aligned, but a text column or a
/// visual bar between two numeric ones must still start at a fixed left edge,
/// which a single split point could not express.
pub fn print_box_row<const N: usize>(
    raw: [&str; N],
    styled: [String; N],
    widths: &[usize],
    right_aligned: [bool; N],
) {
    let edge = dim(glyphs(capabilities()).frame.vertical);
    print_fragment(&format!("  {edge}"));
    for index in 0..N {
        let padding = spaces(widths[index], display_width(raw[index]));
        // A cell arrives already styled, and a caller composing its own row may
        // pass one that never went through a role — the route table's `path`
        // column does. `print_fragment` is what filters those.
        if right_aligned[index] {
            print_fragment(&format!(" {padding}{} {edge}", styled[index]));
        } else {
            print_fragment(&format!(" {}{padding} {edge}", styled[index]));
        }
    }
    print_blank_line();
}

pub fn path_text(path: &Path) -> String {
    paint(path.display().to_string(), "34")
}

pub fn display_path_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

pub fn exists_status(path: &Path) -> String {
    if path.exists() {
        ok_text("ok")
    } else {
        warn_text("missing")
    }
}

pub fn enabled_text(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}

/// A named divider that breaks a long field list into readable groups.
///
/// `doctor` printed twenty-five fields as one block, which is where a reader
/// has to count lines to find the toolchain. The marker in front of the title
/// is what makes the group headings findable by shape at a glance, and the rule
/// fades rather than ending square so it reads as an underline rather than as
/// the top of a box.
pub fn print_section(title: &str) {
    print_blank_line();
    print_line(&section_line(title));
}

pub fn section_line(title: &str) -> String {
    let glyphs = glyphs(capabilities());
    // `"  " + marker + title + " "` precedes the rule, and every rule ends at
    // the same column.
    let used = 2 + display_width(glyphs.marker) + display_width(title) + 1;
    let rule = glyphs.rule.repeat(RULE_END_COLUMN.saturating_sub(used));
    let marker = crate::gradient::BRAND.paint(glyphs.marker);
    let title = label(title);
    if rule.is_empty() {
        // A title wider than the column gets no rule, and must not get the
        // space that would have separated it from one either - an invisible
        // trailing space is still a trailing space in a transcript or a diff.
        return format!("  {marker}{title}");
    }
    format!("  {marker}{title} {}", RULE.paint(rule))
}

/// A free-standing rule, `width` cells wide, fading out to the right. Used
/// under a command header, where there is no title to make room for.
pub fn rule_line(width: usize) -> String {
    RULE.paint(glyphs(capabilities()).rule.repeat(width))
}

/// A horizontal bar sized to `value` against `max`, for comparing rows of a
/// table by eye before reading their numbers.
///
/// Returned unpainted: the caller measures it for the column width, and a
/// string carrying escape sequences measures wrong. [`heat_bar`] is what paints
/// the same cells once the width is settled.
pub fn bar(value: f64, max: f64, width: usize) -> String {
    // `width == 0` is checked with the rest: the clamp below has a minimum of
    // one cell, and `f64::clamp` panics outright when its minimum exceeds its
    // maximum.
    if width == 0 || !(value.is_finite() && max.is_finite()) || max <= 0.0 || value <= 0.0 {
        return String::new();
    }
    let filled = ((value / max) * width as f64)
        .round()
        .clamp(1.0, width as f64) as usize;
    glyphs(capabilities()).bar.repeat(filled)
}

/// Paints a bar produced by [`bar`] along the cool-to-hot ramp.
///
/// Each cell is coloured by where it sits in the **full** column, not in the
/// bar, so a short bar stays green all the way along and only a bar that
/// actually reaches the right-hand edge turns red. Colouring bar-locally would
/// have given the fastest row a red tip.
pub fn heat_bar(cells: &str, width: usize) -> String {
    let depth = color_depth();
    let span = width.max(1).saturating_sub(1).max(1) as f64;
    cells
        .chars()
        .enumerate()
        .map(|(index, cell)| HEAT.cell(depth, &cell.to_string(), index as f64 / span))
        .collect()
}

/// The frame in use, for a caller composing a border this module does not
/// already draw.
pub fn frame() -> Frame {
    glyphs(capabilities()).frame
}
