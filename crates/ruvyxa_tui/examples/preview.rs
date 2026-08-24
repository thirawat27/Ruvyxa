//! Every surface this crate draws, in one run, so a change to how Ruvyxa looks
//! can be *looked at*.
//!
//! ```text
//! cargo run -p ruvyxa_tui --example preview
//! ```
//!
//! This exists because the two most decorated surfaces are the two nobody can
//! see from a test: the spinner and the progress track are gated on both
//! streams being terminals, and the gradients are gated on a colour depth a
//! test process does not report. The unit tests assert everything about them
//! that can be wrong in a way a machine can detect — width, degradation,
//! agreement between the percentage and the count. What they cannot answer is
//! whether it looks good, and reviewing that by rebuilding a demo application
//! and watching thirty pre-render jobs go by is the reason colour changes here
//! used to ship unseen.
//!
//! Worth running under each of the opt-outs too, because each one is a
//! different picture and only this file makes them cheap to compare:
//!
//! ```text
//! NO_COLOR=1     cargo run -p ruvyxa_tui --example preview
//! RUVYXA_ASCII=1 cargo run -p ruvyxa_tui --example preview
//! RUVYXA_FUN=0   cargo run -p ruvyxa_tui --example preview
//! TERM=dumb      cargo run -p ruvyxa_tui --example preview
//! FORCE_COLOR=1  cargo run -p ruvyxa_tui --example preview   # the 16-colour fallback
//! FORCE_COLOR=2  cargo run -p ruvyxa_tui --example preview   # the 256-colour ramp
//! ```

use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

use ruvyxa_tui::{
    BRAND, Capabilities, ColorDepth, ProgressTrack, Spinner, TableRule, accent, alert_text, bar,
    capabilities, column_widths, dim, error_label, heat_bar, info, label, note, number, ok_text,
    path_text, print_box_row, print_field, print_header, print_phase, print_section,
    print_success_banner_at, print_table_rule, rule_line, spinner_line, success, track_line,
    warn_text,
};

fn main() {
    let capabilities = capabilities();

    print_header("Doctor");
    print_field("root", path_text(Path::new("examples/demo")));
    print_field("routes", number("30"));
    print_field("pages", info("27"));
    print_field("api", note("3"));

    print_capabilities(capabilities);
    print_palette();
    print_ramps(capabilities);
    print_table();
    print_phases();
    animate(capabilities);

    print_success_banner_at(
        "Built into",
        Some(Path::new("examples/demo/.ruvyxa")),
        Duration::from_millis(3417),
    );
}

/// What was detected, and therefore which of the pictures below is the one this
/// terminal is getting. Printed first because every surprise further down is
/// explained by this line.
fn print_capabilities(capabilities: Capabilities) {
    print_section("terminal");
    print_field(
        "colour",
        match capabilities.depth {
            ColorDepth::None => warn_text("off"),
            ColorDepth::Ansi16 => accent("16"),
            ColorDepth::Ansi256 => accent("256"),
            ColorDepth::TrueColor => accent("24-bit"),
        },
    );
    print_field(
        "animation",
        if capabilities.animate {
            ok_text("on")
        } else {
            dim("off · not both streams are terminals, or RUVYXA_FUN=0")
        },
    );
    print_field(
        "glyphs",
        if capabilities.unicode {
            accent("unicode")
        } else {
            accent("ascii")
        },
    );
}

/// The roles, side by side. Two that carry different meaning and resolve to the
/// same colour is the failure this row makes obvious.
fn print_palette() {
    print_section("roles");
    print_field(
        "status",
        format!(
            "{} {} {}",
            ok_text("ok"),
            warn_text("warn"),
            alert_text("alert")
        ),
    );
    print_field(
        "values",
        format!(
            "{} {} {} {} {}",
            accent("accent"),
            number("123"),
            info("info"),
            note("note"),
            label("label")
        ),
    );
    print_field("markers", format!("{} {}", success(), error_label()));
}

/// The decorative ramps at full width. On a 16-colour terminal every one of
/// these is a single flat colour, which is the point: nothing disappears.
fn print_ramps(capabilities: Capabilities) {
    print_section("ramps");
    print_field("brand", BRAND.paint("Ruvyxa builds fast websites"));
    print_field("rule", rule_line(28));
    print_field("heat", heat_bar(&bar(1.0, 1.0, 26), 26));
    print_field("heat · low", heat_bar(&bar(0.2, 1.0, 26), 26));
    if capabilities.depth < ColorDepth::Ansi256 {
        print_field(
            "note",
            dim(match capabilities.depth {
                ColorDepth::None => "no colour here, so every ramp above is bare text",
                _ => "16 colours reported here, so every ramp above is one flat role",
            }),
        );
    }
}

fn print_table() {
    print_section("table");
    println!();
    let rows = [
        [
            "route discovery".to_string(),
            "1.20ms".to_string(),
            bar(1.2, 480.0, 12),
        ],
        [
            "boundary analysis".to_string(),
            "18.40ms".to_string(),
            bar(18.4, 480.0, 12),
        ],
        [
            "production build".to_string(),
            "480.00ms".to_string(),
            bar(480.0, 480.0, 12),
        ],
    ];
    let headers = ["Scenario", "Median", "Median share"];
    let widths = column_widths(&headers, &rows);
    let alignment = [false, true, false];

    print_table_rule(&widths, TableRule::Top);
    print_box_row(
        headers,
        [label(headers[0]), label(headers[1]), label(headers[2])],
        &widths,
        alignment,
    );
    print_table_rule(&widths, TableRule::Middle);
    for row in &rows {
        print_box_row(
            [&row[0], &row[1], &row[2]],
            [accent(&row[0]), number(&row[1]), heat_bar(&row[2], 12)],
            &widths,
            alignment,
        );
    }
    print_table_rule(&widths, TableRule::Bottom);
    println!();
}

/// The three ages of a phase line. A build stuck on one phase should not look
/// like a build working through several.
fn print_phases() {
    print_section("phases");
    print_phase("routes discovered", "30 routes".into(), millis(120));
    print_phase("client bundle", "12 chunks".into(), millis(6_400));
    print_phase("prerender", "27 pages".into(), millis(41_000));
}

fn millis(value: u64) -> Duration {
    Duration::from_millis(value)
}

/// The two live surfaces. Skipped entirely when the terminal does not accept
/// animation — which is also what a piped run of this example demonstrates.
fn animate(capabilities: Capabilities) {
    print_section("live");
    if !capabilities.animate {
        // Not "skipped". The frames are exactly what a colour change here needs
        // reviewing, and they are unreachable on the one kind of run - a piped
        // one - where the output can be captured and compared. Three positions
        // are enough to see both ramps and the direction of each.
        print_field("no animation", dim("frames shown as stills instead"));
        for done in [0, 20, 40] {
            println!(
                "{}",
                track_line(capabilities, "prerender", done, 40, 0, None)
            );
        }
        println!(
            "{}",
            spinner_line(capabilities, "bundling", 3, millis(1_800))
        );
        return;
    }

    let spinner = Spinner::start("bundling");
    sleep(Duration::from_millis(1_800));
    spinner.finish("12 chunks".to_string());

    let total = 40;
    let track = ProgressTrack::start(true, "prerender", total);
    for done in 1..=total {
        sleep(Duration::from_millis(70));
        track.set(done);
    }
    drop(track);
    print_phase("prerender", format!("{total} pages"), millis(2_800));
}
