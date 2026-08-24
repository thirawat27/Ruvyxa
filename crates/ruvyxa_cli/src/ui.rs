//! Terminal presentation for the Ruvyxa CLI.
//!
//! Everything shared with the dev server — colour, field layout, tables, the
//! progress track, the mascot, the command header, the success banner, byte and
//! duration formatting — lives in `ruvyxa_tui` and is re-exported here so call
//! sites keep reading `accent(..)` rather than a crate path. What stays in this
//! file is presentation only the CLI has: the tables specific to `bench`,
//! `doctor`, and `check`.
//!
//! The header and the success banner used to live here, and the dev server
//! printed its own copy of the header inline. They are in `ruvyxa_tui::banner`
//! now, which is why the two agree.
//!
//! Nothing here decides anything. Keeping it separate is what stops
//! presentation details from being edited in the same file as build logic.

use std::path::Path;
use std::time::Duration;

// Re-exported rather than imported: sibling modules reach these through
// `crate::*`, and a plain `use` would keep the names private to this file.
pub(crate) use ruvyxa_tui::{
    ProgressTrack, Spinner, TableRule, accent, alert_text, bar, column_widths, dim,
    display_path_relative, display_width, error_label, exists_status, format_bytes,
    format_duration, heat_bar, info, label, note, number, ok_text, path_text, print_box_row,
    print_field, print_header, print_phase, print_section, print_success_banner,
    print_success_banner_at, print_table_rule, spaces, success, warn_text,
};

use crate::commands::BenchmarkResult;

pub(crate) fn print_benchmark_table(
    samples: usize,
    results: &[BenchmarkResult],
    root: &Path,
    app_dir: &Path,
    elapsed: Duration,
) {
    print_header(format!("Benchmark ({samples} sample(s))"));
    print_field("root", path_text(root));
    print_field("app dir", path_text(app_dir));
    print_field("scenarios", number(results.len().to_string()));
    if let Some(result) = results.first() {
        print_field("runtime", info(&result.runtime));
    }
    print_field("duration", accent(format_duration(elapsed)));
    println!();

    // Scenarios in one run differ by orders of magnitude — route discovery in
    // milliseconds against a production build in seconds. The bar is scaled to
    // the slowest median so the shape of that gap is visible before the numbers
    // are read.
    let slowest_median = results
        .iter()
        .map(|result| result.median_ms)
        .fold(0.0_f64, f64::max);
    let rows = results
        .iter()
        .map(|result| {
            [
                result.name.clone(),
                format!("{:.2}ms", result.min_ms),
                format!("{:.2}ms", result.median_ms),
                format!("{:.2}ms", result.avg_ms),
                format!("{:.2}ms", result.max_ms),
                bar(result.median_ms, slowest_median, BENCHMARK_BAR_WIDTH),
            ]
        })
        .collect::<Vec<_>>();
    let headers = ["Scenario", "Min", "Median", "Avg", "Max", "Median share"];
    let widths = column_widths(&headers, &rows);

    print_table_rule(&widths, TableRule::Top);
    print_box_row(
        headers,
        [
            label(headers[0]),
            label(headers[1]),
            label(headers[2]),
            label(headers[3]),
            label(headers[4]),
            label(headers[5]),
        ],
        &widths,
        BENCHMARK_ALIGNMENT,
    );
    print_table_rule(&widths, TableRule::Middle);

    for row in rows {
        print_box_row(
            [&row[0], &row[1], &row[2], &row[3], &row[4], &row[5]],
            [
                accent(&row[0]),
                ok_text(&row[1]),
                number(&row[2]),
                info(&row[3]),
                warn_text(&row[4]),
                // Coloured against the full column rather than against its own
                // length, so only a row that actually reaches the right edge
                // reads as the expensive one.
                heat_bar(&row[5], BENCHMARK_BAR_WIDTH),
            ],
            &widths,
            BENCHMARK_ALIGNMENT,
        );
    }
    print_table_rule(&widths, TableRule::Bottom);
}

const BENCHMARK_BAR_WIDTH: usize = 12;

/// Scenario name left, the four timings right, and the bar left again so every
/// bar grows from the same edge.
const BENCHMARK_ALIGNMENT: [bool; 6] = [false, true, true, true, true, false];

pub(crate) fn tool_status(value: String) -> String {
    if value == "missing" {
        warn_text(value)
    } else {
        ok_text(value)
    }
}

pub(crate) fn compatibility_status(value: String) -> String {
    if value.starts_with("ok ") {
        ok_text(value)
    } else {
        warn_text(value)
    }
}
