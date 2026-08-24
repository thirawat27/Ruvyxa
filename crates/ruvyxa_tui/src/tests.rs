use std::time::Duration;

use crate::*;

fn env_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + use<'a> {
    move |name| {
        pairs
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.to_string())
    }
}

#[test]
fn a_terminal_gets_colour_animation_and_unicode() {
    // Asked as a non-Windows console, because a Windows one answers TrueColor
    // whatever the environment says and would decide this on the platform the
    // suite happens to run on rather than on the rule being asserted.
    let capabilities = detect_capabilities_on(true, true, env_from(&[]), false);
    assert_eq!(
        capabilities,
        Capabilities {
            color: true,
            // Nothing in the environment claims more, so the depth every
            // terminal is known to have is what it gets. Roles render
            // identically at every depth; only decoration notices.
            depth: ColorDepth::Ansi16,
            animate: true,
            unicode: true
        }
    );
}

#[test]
fn a_pipe_gets_no_colour_and_no_movement_but_keeps_its_glyphs() {
    assert_eq!(
        detect_capabilities(false, false, env_from(&[])),
        Capabilities {
            color: false,
            depth: ColorDepth::None,
            animate: false,
            unicode: true
        }
    );
}

#[test]
fn a_captured_stdout_keeps_animation_off_even_with_a_terminal_on_stderr() {
    // `ruvyxa build > log`: the results are being captured, so the run reports
    // one line per event rather than painting frames on the other stream.
    let capabilities = detect_capabilities(false, true, env_from(&[]));
    assert!(!capabilities.animate);
    assert!(!capabilities.color);
}

#[test]
fn a_captured_stderr_stops_the_animation_it_would_be_written_to() {
    // `ruvyxa build 2> log`: frames go to stderr, so a redirected stderr means
    // no frames — colour on stdout is unaffected.
    let capabilities = detect_capabilities(true, false, env_from(&[]));
    assert!(!capabilities.animate);
    assert!(capabilities.color);
}

#[test]
fn no_color_keeps_animation_but_drops_colour() {
    let capabilities = detect_capabilities(true, true, env_from(&[("NO_COLOR", "1")]));
    assert!(!capabilities.color);
    assert!(capabilities.animate);
}

#[test]
fn dumb_terminals_get_nothing() {
    assert_eq!(
        detect_capabilities(true, true, env_from(&[("TERM", "dumb")])),
        Capabilities::PLAIN
    );
}

#[test]
fn ruvyxa_fun_off_keeps_colour_and_stops_movement() {
    for value in ["0", "false", "off", "no", ""] {
        let capabilities = detect_capabilities(true, true, env_from(&[("RUVYXA_FUN", value)]));
        assert!(
            capabilities.color,
            "colour should survive RUVYXA_FUN={value}"
        );
        assert!(
            !capabilities.animate,
            "RUVYXA_FUN={value} should stop movement"
        );
    }
}

#[test]
fn ruvyxa_fun_on_leaves_animation_enabled() {
    assert!(detect_capabilities(true, true, env_from(&[("RUVYXA_FUN", "1")])).animate);
}

#[test]
fn ruvyxa_ascii_selects_the_ascii_glyph_set() {
    let capabilities = detect_capabilities(true, true, env_from(&[("RUVYXA_ASCII", "1")]));
    assert!(!capabilities.unicode);
    assert_eq!(glyphs(capabilities), ASCII_GLYPHS);
}

#[test]
fn paint_wraps_only_when_colour_is_allowed() {
    assert_eq!(paint_when(true, "build", "36"), "\x1b[36mbuild\x1b[0m");
    assert_eq!(paint_when(false, "build", "36"), "build");
}

#[test]
fn the_runner_starts_at_the_left_edge() {
    let (behind, runner, ahead) = runner_cells(UNICODE_GLYPHS, 0, 10, 0);
    assert_eq!(behind, "");
    assert_eq!(runner, "🦊");
    assert_eq!(ahead.chars().count(), TRACK_WIDTH);
}

#[test]
fn the_runner_reaches_the_right_edge_when_the_work_is_done() {
    let (behind, _, ahead) = runner_cells(UNICODE_GLYPHS, 10, 10, 0);
    assert_eq!(behind.chars().count(), TRACK_WIDTH);
    assert_eq!(ahead, "");
}

#[test]
fn the_track_keeps_one_width_at_every_position() {
    for done in 0..=10 {
        let (behind, _, ahead) = runner_cells(UNICODE_GLYPHS, done, 10, 0);
        assert_eq!(
            behind.chars().count() + ahead.chars().count(),
            TRACK_WIDTH,
            "track width changed at {done}/10"
        );
    }
}

#[test]
fn dust_alternates_behind_the_runner_between_frames() {
    let (first, _, _) = runner_cells(UNICODE_GLYPHS, 5, 10, 0);
    let (second, _, _) = runner_cells(UNICODE_GLYPHS, 5, 10, 1);
    assert_ne!(first, second);
    assert!(first.ends_with(UNICODE_GLYPHS.dust[0]));
    assert!(second.ends_with(UNICODE_GLYPHS.dust[1]));
}

#[test]
fn overshooting_the_total_does_not_panic_or_overflow_the_track() {
    let (behind, _, ahead) = runner_cells(UNICODE_GLYPHS, 99, 10, 0);
    assert_eq!(behind.chars().count(), TRACK_WIDTH);
    assert_eq!(ahead, "");
}

#[test]
fn a_zero_total_leaves_the_track_empty() {
    let (behind, _, ahead) = runner_cells(UNICODE_GLYPHS, 3, 0, 0);
    assert_eq!(behind, "");
    assert_eq!(ahead.chars().count(), TRACK_WIDTH);
}

#[test]
fn field_and_phase_lines_put_their_value_in_the_same_column() {
    // The alignment the two widths exist to produce: a field line and a phase
    // line read as one table.
    //
    // Measured on the visible text. Counting the raw string instead compared a
    // number that includes the CSI sequences `paint` wraps values in, and those
    // are not the same on both lines: `phase_line` colours its detail, so five
    // characters of `\x1b[36m` sit immediately before the value, while
    // `field_line` emits the value bare. That made this assertion pass only
    // when colour was off — which is every piped run and no interactive one, so
    // it held in CI and failed on the developer's terminal while never checking
    // the alignment that colour users actually see.
    fn value_column(line: &str) -> usize {
        let visible = strip_ansi(line);
        visible[..visible.find("value").expect("value is present")]
            .chars()
            .count()
    }

    let field = value_column(&field_line("app dir", "value".to_string()));
    assert_eq!(
        field,
        value_column(&phase_line(
            "routes discovered",
            "value".to_string(),
            Duration::ZERO
        )),
        "field and phase lines disagree on the value column"
    );
    // Pinned, so a change to either width has to be a deliberate edit here
    // rather than two mistakes that happen to cancel out. This is the column
    // the module documentation promises.
    assert_eq!(field, 25, "both lines should put their value in column 25");
}

#[test]
fn header_title_keeps_the_mascot_in_piped_output() {
    assert_eq!(tui_header_title("Build"), "🦊 Ruvyxa Build");
}

#[test]
fn each_command_gets_its_own_badge_and_a_fallback_covers_the_rest() {
    assert_eq!(badge("Doctor").icon, "🩺");
    assert_eq!(badge("Clean").icon, "🧹");
    // Resolved on the first word, so a parameterised title still matches.
    assert_eq!(
        badge("Benchmark (3 sample(s))").icon,
        badge("Benchmark").icon
    );
    assert_eq!(badge("Something New").icon, "🦊");
}

#[test]
fn every_badge_is_distinguishable_from_the_others() {
    let mut icons = BADGES
        .iter()
        .map(|(_, badge)| badge.icon)
        .collect::<Vec<_>>();
    icons.sort_unstable();
    let total = icons.len();
    icons.dedup();
    assert_eq!(icons.len(), total, "two commands share an icon");
}

#[test]
fn a_bar_scales_to_the_largest_value_and_never_disappears() {
    assert_eq!(bar(10.0, 10.0, 10).chars().count(), 10);
    assert_eq!(bar(5.0, 10.0, 10).chars().count(), 5);
    // A value orders of magnitude below the maximum still gets one cell, so a
    // fast row reads as fast rather than as missing.
    assert_eq!(bar(0.01, 1000.0, 10).chars().count(), 1);
}

#[test]
fn a_bar_with_nothing_to_show_is_empty_rather_than_wrong() {
    assert!(bar(0.0, 10.0, 10).is_empty());
    assert!(bar(5.0, 0.0, 10).is_empty());
    assert!(bar(f64::NAN, 10.0, 10).is_empty());
    assert!(bar(1.0, f64::INFINITY, 10).is_empty());
}

#[test]
fn roles_that_share_a_column_get_different_colours() {
    // `info` and `note` sit in the same column in the route table and the
    // benchmark table; identical codes would make the distinction invisible.
    let codes = [
        HEADING_CODE,
        "1;33",
        "36",
        "1;96",
        "94",
        "95",
        "32",
        "33",
        "31",
        "34",
        "90",
    ];
    let painted = codes
        .iter()
        .map(|code| paint_when(true, "x", code))
        .collect::<Vec<_>>();
    for (index, first) in painted.iter().enumerate() {
        for second in painted.iter().skip(index + 1) {
            assert_ne!(first, second, "two palette roles resolve to one colour");
        }
    }
}

#[test]
fn durations_switch_units_at_one_second() {
    assert_eq!(format_duration(Duration::from_millis(120)), "120ms");
    assert_eq!(format_duration(Duration::from_millis(1500)), "1.50s");
}

#[test]
fn byte_sizes_switch_units_at_each_boundary() {
    assert_eq!(format_bytes(512), "512 B");
    assert_eq!(format_bytes(2048), "2.0 kB");
    assert_eq!(format_bytes(2 * 1024 * 1024), "2.0 MB");
}

#[test]
fn a_spinner_on_a_non_animating_terminal_still_reports_its_phase() {
    // No terminal in the test process, so no thread is spawned and `finish`
    // degrades to the plain phase line.
    let spinner = Spinner::start("bundling");
    spinner.finish("12 chunks".to_string());
}

/// Remove CSI sequences so a rendered line can be measured in visible columns.
///
/// Terminal styling is invisible width: `\x1b[36mvalue\x1b[0m` occupies five
/// columns, not fourteen. Any assertion about where a value lands on screen has
/// to strip it first, or it silently measures something else whenever colour is
/// enabled — which is exactly the environment the alignment is for.
///
/// Only the `\x1b[…<final>` form is handled, because that is the only form
/// `paint` emits.
fn strip_ansi(value: &str) -> String {
    let mut visible = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character != '\x1b' {
            visible.push(character);
            continue;
        }
        if chars.next() != Some('[') {
            continue;
        }
        // Parameter and intermediate bytes run until the final byte in 0x40..=0x7E.
        for parameter in chars.by_ref() {
            if ('\u{40}'..='\u{7e}').contains(&parameter) {
                break;
            }
        }
    }
    visible
}

#[test]
fn stripping_ansi_leaves_only_what_the_terminal_shows() {
    assert_eq!(strip_ansi("\x1b[36mvalue\x1b[0m"), "value");
    assert_eq!(strip_ansi("plain"), "plain");
    assert_eq!(strip_ansi("\x1b[1;96m7\x1b[0m routes"), "7 routes");
    // A multi-byte glyph is content, not styling.
    assert_eq!(strip_ansi("\x1b[32m✓\x1b[0m ok"), "✓ ok");
}
