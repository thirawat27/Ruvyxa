//! Field, table, and unit formatting shared by every Ruvyxa command.
//!
//! The two label widths below are chosen together, not independently. A field
//! line is `"  " + label(22) + " "` and a phase line is `"  ✓ " + label(20) +
//! " "`, so both put their value in column 25 and the build summary reads as
//! one table instead of two. Changing one without the other reintroduces the
//! misalignment this module exists to remove. The width itself is set by the
//! longest label any command prints — `dependency duplicates`, from `doctor`.

use std::path::Path;
use std::time::Duration;

use chrono::Local;

use crate::theme::{dim, label, ok_text, paint, warn_text};

/// Width of the label column in a `key: value` field line.
pub const FIELD_LABEL_WIDTH: usize = 22;

/// Width of the label column in a build-phase line, which carries a two-column
/// status glyph before the label.
pub const PHASE_LABEL_WIDTH: usize = 20;

pub fn print_field(name: &str, value: String) {
    println!("{}", field_line(name, value));
}

pub fn field_line(name: &str, value: String) -> String {
    format!(
        "  {}{} {}",
        label(name),
        spaces(FIELD_LABEL_WIDTH, name.len()),
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

pub fn print_table_separator(widths: &[usize]) {
    print!("  {}", dim("+"));
    for width in widths {
        print!("{}", dim("-".repeat(*width + 2)));
        print!("{}", dim("+"));
    }
    println!();
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
    print!("  {}", dim("|"));
    for index in 0..N {
        if !right_aligned[index] {
            print!(
                " {}{} {}",
                styled[index],
                spaces(widths[index], display_width(raw[index])),
                dim("|")
            );
        } else {
            print!(
                " {}{} {}",
                spaces(widths[index], display_width(raw[index])),
                styled[index],
                dim("|")
            );
        }
    }
    println!();
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
/// has to count lines to find the toolchain. The rule is drawn to the same
/// width as the field column so the groups line up with the values they cover.
pub fn print_section(title: &str) {
    println!();
    println!("{}", section_line(title));
}

pub fn section_line(title: &str) -> String {
    let dashes = (FIELD_LABEL_WIDTH + 8).saturating_sub(title.chars().count() + 3);
    format!("  {} {}", label(title), dim("─".repeat(dashes)))
}

/// A horizontal bar sized to `value` against `max`, for comparing rows of a
/// table by eye before reading their numbers.
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
    "▇".repeat(filled)
}
