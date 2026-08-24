//! Tests for the parts of the command line nobody can see from a test process.
//!
//! Everything animated is gated on both streams being terminals, and everything
//! decorative is gated on a colour depth the test process does not have. That
//! is exactly why the drawing was split into pure line builders — `track_line`,
//! `spinner_line`, `table_rule_line`, `section_line`, `Gradient::paint_with` —
//! and it is those, not the printing wrappers, that are asserted here.
//!
//! Two properties are worth more than the rest and are checked at every
//! position rather than at a sample: a line must fit in eighty columns, and a
//! ramp must degrade to its role rather than to nothing.

use std::time::Duration;

use crate::*;

/// The narrowest terminal worth supporting. A frame that exceeds it wraps, and
/// a wrapped frame is never erased cleanly — `\x1b[2K` clears one line.
const NARROWEST_TERMINAL: usize = 80;

fn env_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + use<'a> {
    move |name| {
        pairs
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.to_string())
    }
}

fn full_colour() -> Capabilities {
    Capabilities {
        color: true,
        depth: ColorDepth::TrueColor,
        animate: true,
        unicode: true,
    }
}

/// Remove CSI sequences so a rendered line can be measured in visible columns.
/// Mirrors the helper in the sibling test module; see the note there.
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
        for parameter in chars.by_ref() {
            if ('\u{40}'..='\u{7e}').contains(&parameter) {
                break;
            }
        }
    }
    visible
}

/// Visible columns, counting the mascot as the two cells a terminal gives it.
///
/// `display_width` counts characters, which is right for the tables — they hold
/// no emoji. The progress track holds exactly one, and a line measured one
/// column short is a line that wraps on the terminal it was sized for.
fn columns(line: &str) -> usize {
    let visible = strip_ansi(line);
    visible.chars().count() + visible.matches(MASCOT).count()
}

// ─── Colour depth ────────────────────────────────────────────────────────────

#[test]
fn colour_and_depth_never_disagree() {
    // `color` is a convenience over `depth`, and a call site reading one while
    // another reads the other must not be able to see two different answers.
    let environments: [&[(&str, &str)]; 6] = [
        &[],
        &[("COLORTERM", "truecolor")],
        &[("TERM", "xterm-256color")],
        &[("NO_COLOR", "1")],
        &[("TERM", "dumb")],
        &[("FORCE_COLOR", "2")],
    ];
    for pairs in environments {
        for stdout in [true, false] {
            let capabilities = detect_capabilities(stdout, true, env_from(pairs));
            assert_eq!(
                capabilities.color,
                capabilities.depth != ColorDepth::None,
                "colour and depth disagree for {pairs:?} with stdout={stdout}"
            );
        }
    }
}

#[test]
fn a_terminal_that_advertises_more_colour_is_believed() {
    // Every case below is a claim the *environment* makes, so the platform is
    // pinned to a non-Windows console: a Windows one short-circuits to
    // TrueColor and would answer all of these without reading a variable.
    let depth =
        |pairs: &[(&str, &str)]| detect_capabilities_on(true, true, env_from(pairs), false).depth;

    assert_eq!(depth(&[("COLORTERM", "truecolor")]), ColorDepth::TrueColor);
    assert_eq!(depth(&[("COLORTERM", "24bit")]), ColorDepth::TrueColor);
    // Windows Terminal sets no COLORTERM on a native shell.
    assert_eq!(depth(&[("WT_SESSION", "abc")]), ColorDepth::TrueColor);
    assert_eq!(
        depth(&[("TERM_PROGRAM", "iTerm.app")]),
        ColorDepth::TrueColor
    );
    assert_eq!(depth(&[("TERM_PROGRAM", "vscode")]), ColorDepth::TrueColor);
    assert_eq!(depth(&[("TERM", "xterm-256color")]), ColorDepth::Ansi256);
    assert_eq!(depth(&[("TERM", "xterm")]), ColorDepth::Ansi16);
    assert_eq!(depth(&[]), ColorDepth::Ansi16);
}

#[test]
fn an_unrelated_program_is_not_promoted_by_a_partial_name() {
    // Matched whole, so a terminal called `vscode-insiders-something` does not
    // inherit the claim by containing a known name.
    assert_eq!(
        detect_capabilities_on(
            true,
            true,
            env_from(&[("TERM_PROGRAM", "vscode-ish")]),
            false
        )
        .depth,
        ColorDepth::Ansi16
    );
}

#[test]
fn a_windows_console_is_taken_as_24_bit_when_nothing_else_says_otherwise() {
    // A native PowerShell session sets none of the Unix-world variables, so the
    // table fell through to Ansi16 and flattened every gradient in the product
    // on the platform this framework is most often developed on.
    assert_eq!(
        detect_capabilities_on(true, true, env_from(&[]), true).depth,
        ColorDepth::TrueColor
    );
    // Including when an emulation layer has exported a `TERM` describing
    // itself rather than the window drawing the pixels — Git Bash exports
    // `xterm`, which is not what Windows Terminal can do.
    assert_eq!(
        detect_capabilities_on(true, true, env_from(&[("TERM", "xterm")]), true).depth,
        ColorDepth::TrueColor
    );
    // The opt-outs still outrank it: this branch answers *how much* colour,
    // never *whether*.
    assert!(!detect_capabilities_on(true, true, env_from(&[("NO_COLOR", "1")]), true).color);
    assert!(!detect_capabilities_on(false, false, env_from(&[]), true).color);
}

#[test]
fn forcing_colour_paints_a_pipe_but_never_animates_it() {
    // The case this exists for: a CI log that renders ANSI. Colour is wanted;
    // repainting a line that has nowhere to repaint to is not.
    let capabilities = detect_capabilities(false, false, env_from(&[("FORCE_COLOR", "1")]));
    assert!(capabilities.color);
    assert_eq!(capabilities.depth, ColorDepth::Ansi16);
    assert!(!capabilities.animate);
}

#[test]
fn forcing_colour_pins_the_depth_by_level() {
    let depth =
        |value: &str| detect_capabilities(false, false, env_from(&[("FORCE_COLOR", value)])).depth;
    assert_eq!(depth("1"), ColorDepth::Ansi16);
    assert_eq!(depth("2"), ColorDepth::Ansi256);
    assert_eq!(depth("3"), ColorDepth::TrueColor);
    // Anything else set and not negative means "as much as you have".
    assert_eq!(depth("yes"), ColorDepth::TrueColor);
}

#[test]
fn force_colour_zero_is_how_the_same_variable_says_no() {
    let capabilities = detect_capabilities(true, true, env_from(&[("FORCE_COLOR", "0")]));
    assert!(!capabilities.color);
    // Still a terminal, so movement is unaffected: this variable is about
    // colour and says nothing about animation.
    assert!(capabilities.animate);
}

#[test]
fn an_explicit_force_outranks_the_ambient_opt_outs() {
    // NO_COLOR and TERM are usually inherited from a shell profile or a CI
    // image; FORCE_COLOR is set for this run, on purpose.
    for ambient in [("NO_COLOR", "1"), ("TERM", "dumb")] {
        let capabilities =
            detect_capabilities(false, false, env_from(&[ambient, ("FORCE_COLOR", "3")]));
        assert!(
            capabilities.color,
            "FORCE_COLOR should outrank {}",
            ambient.0
        );
        assert_eq!(capabilities.depth, ColorDepth::TrueColor);
    }
}

#[test]
fn clicolor_force_is_accepted_as_the_same_request() {
    assert!(detect_capabilities(false, false, env_from(&[("CLICOLOR_FORCE", "1")])).color);
}

// ─── Gradients ───────────────────────────────────────────────────────────────

#[test]
fn a_ramp_collapses_to_its_role_rather_than_to_nothing() {
    // The whole reason decoration is allowed 24-bit colour: on a terminal that
    // cannot render it, the text is still painted — just in one colour.
    for gradient in [BRAND, TRAIL, RULE, HEAT, PULSE] {
        let flat = gradient.paint_with(ColorDepth::Ansi16, "value");
        assert_eq!(
            flat,
            paint_when(true, "value", gradient.fallback),
            "a 16-colour terminal should get the fallback role"
        );
        assert_eq!(
            gradient.paint_with(ColorDepth::None, "value"),
            "value",
            "no colour means no escapes"
        );
    }
}

#[test]
fn a_ramp_spends_a_distinct_colour_on_each_character() {
    let painted = BRAND.paint_with(ColorDepth::TrueColor, "Ruvyxa");
    assert_eq!(strip_ansi(&painted), "Ruvyxa");
    // Six characters, six colour changes, one reset.
    assert_eq!(painted.matches("\x1b[1;38;2;").count(), 6);
    assert!(painted.ends_with("\x1b[0m"));
}

#[test]
fn the_wordmark_degrades_to_the_heading_colour_it_replaced() {
    // A sixteen-colour terminal should see exactly the header it saw before the
    // ramp existed. That only holds while these two agree, and they are written
    // in three places - here, in clap's `--help` styles, and in the palette -
    // so one of them has to be the anchor.
    assert_eq!(BRAND.fallback, HEADING_CODE);
    assert_eq!(
        BRAND.paint_with(ColorDepth::Ansi16, "Ruvyxa Build"),
        paint_when(true, "Ruvyxa Build", HEADING_CODE)
    );
}

#[test]
fn a_ramp_reaches_both_of_its_ends() {
    let first = BRAND.stops[0];
    let last = BRAND.stops[BRAND.stops.len() - 1];
    assert_eq!(BRAND.sample(0.0), first);
    assert_eq!(BRAND.sample(1.0), last);
    // Out of range is clamped rather than wrapped or extrapolated.
    assert_eq!(BRAND.sample(-4.0), first);
    assert_eq!(BRAND.sample(9.0), last);
}

#[test]
fn a_single_character_takes_the_start_of_the_ramp_rather_than_dividing_by_zero() {
    let painted = BRAND.paint_with(ColorDepth::TrueColor, "R");
    let Rgb(red, green, blue) = BRAND.stops[0];
    assert!(painted.starts_with(&format!("\x1b[1;38;2;{red};{green};{blue}m")));
}

#[test]
fn an_empty_string_paints_to_nothing() {
    assert_eq!(BRAND.paint_with(ColorDepth::TrueColor, ""), "");
}

#[test]
fn a_space_is_left_unpainted_because_a_coloured_space_looks_identical() {
    let painted = BRAND.paint_with(ColorDepth::TrueColor, "a b");
    assert_eq!(strip_ansi(&painted), "a b");
    // The run closes before the gap and reopens after it, so the space carries
    // no colour of its own.
    let gap = painted.find(' ').expect("the space survives");
    assert!(painted[..gap].ends_with("\x1b[0m"));
}

#[test]
fn a_256_colour_terminal_gets_indexed_codes_and_not_24_bit_ones() {
    let painted = RULE.paint_with(ColorDepth::Ansi256, "───");
    assert!(painted.contains("\x1b[38;5;"));
    assert!(!painted.contains("38;2;"));
}

#[test]
fn cycling_a_ramp_moves_the_colour_without_moving_the_text() {
    let still = PULSE.paint_cycled_with(ColorDepth::TrueColor, "⠋", 0.0);
    let later = PULSE.paint_cycled_with(ColorDepth::TrueColor, "⠋", 0.25);
    assert_ne!(still, later, "the shimmer should advance with the phase");
    assert_eq!(strip_ansi(&still), strip_ansi(&later));
}

#[test]
fn a_cycled_ramp_meets_itself_so_the_shimmer_does_not_jump() {
    // The first and last stops of PULSE are the same colour on purpose: a phase
    // of 0.0 and a phase of one full turn must land on the same frame.
    assert_eq!(PULSE.stops[0], PULSE.stops[PULSE.stops.len() - 1]);
    assert_eq!(
        PULSE.paint_cycled_with(ColorDepth::TrueColor, "⠋", 0.0),
        PULSE.paint_cycled_with(ColorDepth::TrueColor, "⠋", 1.0)
    );
}

#[test]
fn a_neutral_quantises_through_the_grey_ramp_and_not_the_cube() {
    // Through the 6×6×6 cube a fade between greys snaps to six levels, which
    // reads as three visible steps rather than as a fade.
    assert_eq!(to_ansi256(Rgb(0, 0, 0)), 16);
    assert_eq!(to_ansi256(Rgb(255, 255, 255)), 231);
    let mid = to_ansi256(Rgb(128, 128, 128));
    assert!(
        (232..=255).contains(&mid),
        "expected a grey-ramp index, got {mid}"
    );
    // A colour is not a neutral and belongs in the cube.
    assert!((16..232).contains(&to_ansi256(Rgb(255, 0, 0))));
}

// ─── Rules and frames ────────────────────────────────────────────────────────

#[test]
fn every_rule_ends_at_the_same_column() {
    // A screen of `doctor` output carries five of these. Five lengths chosen
    // independently look like five mistakes.
    for title in ["ruvyxa", "project", "toolchain", "adapter", "graph"] {
        assert_eq!(
            columns(&section_line(title)),
            RULE_END_COLUMN,
            "section rule for {title} stops in the wrong column"
        );
    }
}

#[test]
fn a_title_too_long_for_its_rule_shortens_the_rule_instead_of_overflowing() {
    let line = strip_ansi(&section_line(
        "a section title far wider than the column allows",
    ));
    assert!(!line.contains('─'));
    // And drops the separator with it. An invisible trailing space still shows
    // up in a transcript, in a diff, and in anything that trims lines.
    assert_eq!(line.trim_end(), line, "the line ends in whitespace");
}

#[test]
fn a_section_title_is_measured_in_characters_too() {
    // The title sits between the marker and the rule, so counting its bytes
    // would shorten the rule by one cell for every non-ASCII character in it.
    assert_eq!(
        columns(&section_line("what each row measures")),
        RULE_END_COLUMN
    );
    assert_eq!(
        columns(&section_line("what each \u{b7} measures")),
        RULE_END_COLUMN
    );
}

#[test]
fn a_table_is_opened_joined_and_closed_with_three_different_rules() {
    let widths = [4, 8];
    let top = table_rule_line(&widths, TableRule::Top);
    let middle = table_rule_line(&widths, TableRule::Middle);
    let bottom = table_rule_line(&widths, TableRule::Bottom);
    assert_ne!(top, middle);
    assert_ne!(middle, bottom);
    assert_ne!(top, bottom);
    // A frame is only correct as a set: all three have to measure the same, or
    // the table is a parallelogram.
    assert_eq!(columns(&top), columns(&middle));
    assert_eq!(columns(&middle), columns(&bottom));
}

#[test]
fn a_rule_is_as_wide_as_the_row_it_frames() {
    // `"  " + edge + per column (" " + cell + padding + " " + edge)`.
    let widths = [4, 8, 2];
    let expected = 2 + 1 + widths.iter().map(|width| width + 3).sum::<usize>();
    assert_eq!(columns(&table_rule_line(&widths, TableRule::Top)), expected);
}

#[test]
fn an_ascii_terminal_gets_a_complete_frame_rather_than_a_mixed_one() {
    // Every corner and join of both sets is present, so a table cannot be
    // opened with box drawing and closed with plus signs.
    for frame in [UNICODE_GLYPHS.frame, ASCII_GLYPHS.frame] {
        for part in [
            frame.top_left,
            frame.top_join,
            frame.top_right,
            frame.mid_left,
            frame.mid_join,
            frame.mid_right,
            frame.bottom_left,
            frame.bottom_join,
            frame.bottom_right,
            frame.horizontal,
            frame.vertical,
        ] {
            assert!(!part.is_empty(), "a frame part is missing");
        }
    }
    assert!(ASCII_GLYPHS.frame.top_left.is_ascii());
    assert!(!UNICODE_GLYPHS.frame.top_left.is_ascii());
}

#[test]
fn every_glyph_in_both_sets_is_drawable() {
    for set in [UNICODE_GLYPHS, ASCII_GLYPHS] {
        for glyph in [
            set.filled,
            set.empty,
            set.dust[0],
            set.dust[1],
            set.runner,
            set.done,
            set.failed,
            set.pending,
            set.sparkle,
            set.rule,
            set.marker,
            set.bar,
        ] {
            assert!(!glyph.is_empty(), "an empty glyph would draw nothing");
        }
        assert!(!set.spinner.is_empty());
    }
}

#[test]
fn a_passing_step_and_a_failing_one_are_told_apart_by_glyph_and_by_colour() {
    assert_ne!(UNICODE_GLYPHS.done, UNICODE_GLYPHS.failed);
    assert_ne!(ASCII_GLYPHS.done, ASCII_GLYPHS.failed);
    assert_ne!(success(), error_label());
}

#[test]
fn a_label_is_padded_by_characters_and_not_by_bytes() {
    // Every label the CLI prints today is ASCII, where the two agree. The
    // preview example prints one with a middle dot in it, and it landed a
    // column short of the others until the padding stopped counting bytes.
    let ascii = field_line("heat low", "value".to_string());
    let wide = field_line("heat \u{b7} low", "value".to_string());
    assert_eq!(value_column(&ascii), value_column(&wide));
}

#[test]
fn a_phase_label_is_padded_the_same_way() {
    let ascii = phase_line("client bundle", "value".to_string(), Duration::ZERO);
    let wide = phase_line("client \u{b7} bundle", "value".to_string(), Duration::ZERO);
    // Both labels are shorter than the column, so the padding is what makes
    // them land together. Counting bytes made the wider label two columns
    // short of the other, which is the misalignment the column exists to stop.
    assert_eq!(value_column(&ascii), value_column(&wide));
}

/// The column the word `value` starts in, measured on the visible text.
fn value_column(line: &str) -> usize {
    let visible = strip_ansi(line);
    visible[..visible.find("value").expect("value is present")]
        .chars()
        .count()
}

// ─── Badges ──────────────────────────────────────────────────────────────────

/// Code points that default to *text* presentation and therefore need U+FE0F to
/// be drawn as emoji at all.
///
/// This list is knowledge, not derivation: nothing in the standard library
/// exposes `Emoji_Presentation`, so the check below is exactly as good as what
/// is written here. It covers every such code point the badge table has used,
/// including one it no longer does — U+1F5FA was replaced by a compass rather
/// than fixed, and leaving it listed is what stops it coming back bare.
const TEXT_PRESENTATION_DEFAULTS: [char; 3] = ['\u{1F5FA}', '\u{2696}', '\u{23F1}'];

#[test]
fn an_icon_that_defaults_to_text_asks_for_emoji_presentation() {
    // Without the selector a terminal draws the glyph at emoji size and then
    // advances one column, so the tagline is printed on top of it. That is what
    // welded `dev and prod must agree` to its own scales.
    for (command, badge) in BADGES {
        let base = badge.icon.chars().next().expect("an icon is never empty");
        if TEXT_PRESENTATION_DEFAULTS.contains(&base) {
            assert!(
                badge.icon.contains('\u{FE0F}'),
                "{command}'s icon defaults to text presentation and needs U+FE0F"
            );
        }
    }
}

#[test]
fn an_icon_is_one_glyph_and_nothing_else() {
    // A base character, optionally followed by the presentation selector. Two
    // emoji in one badge would silently widen the line the taglines align to.
    for (command, badge) in BADGES {
        let stripped = badge.icon.replace('\u{FE0F}', "");
        assert_eq!(
            stripped.chars().count(),
            1,
            "{command}'s icon is not a single glyph"
        );
    }
}

#[test]
fn a_badge_leaves_two_columns_between_its_icon_and_its_tagline() {
    // One column is enough only on a terminal whose cursor advance agrees with
    // what it drew, and the terminal that reported this bug did not.
    for (command, badge) in BADGES {
        let line = strip_ansi(&badge_line(command));
        let after_icon = line
            .split_once(badge.icon)
            .expect("the line carries its icon")
            .1;
        assert!(
            after_icon.starts_with("  ") && !after_icon.starts_with("   "),
            "{command}'s badge does not leave exactly two columns after its icon"
        );
    }
}

#[test]
fn a_tagline_carries_no_padding_of_its_own() {
    // The gap belongs to the line that draws it. A leading space in the data
    // was the first attempt at this bug, and it puts the fix somewhere nobody
    // looking at the layout would think to check.
    for (command, badge) in BADGES {
        assert_eq!(
            badge.tagline.trim(),
            badge.tagline,
            "{command}'s tagline carries padding that belongs in `badge_line`"
        );
    }
}

// ─── The progress track ──────────────────────────────────────────────────────

#[test]
fn a_progress_frame_fits_the_narrowest_terminal_at_every_position() {
    for done in 0..=200 {
        let line = track_line(
            full_colour(),
            "prerender",
            done,
            200,
            done,
            Some(Duration::from_millis(1234)),
        );
        let width = columns(&line);
        assert!(
            width <= NARROWEST_TERMINAL,
            "the track is {width} columns wide at {done}/200"
        );
    }
}

#[test]
fn a_progress_frame_keeps_one_width_while_the_runner_advances() {
    // A frame that changes width leaves the tail of the previous one behind on
    // any terminal that does not erase, and jitters on one that does.
    let widths = (0..=30)
        .map(|done| columns(&track_line(full_colour(), "prerender", done, 30, 0, None)))
        .collect::<Vec<_>>();
    assert!(
        widths.windows(2).all(|pair| pair[0] == pair[1]),
        "track width changed as the runner advanced: {widths:?}"
    );
}

#[test]
fn the_percentage_agrees_with_the_count_beside_it() {
    let visible = strip_ansi(&track_line(full_colour(), "prerender", 13, 30, 0, None));
    assert!(visible.contains(" 43%"), "expected 43% in {visible:?}");
    assert!(
        visible.contains("13/30"),
        "expected the count in {visible:?}"
    );
}

#[test]
fn an_estimate_is_omitted_until_there_is_something_to_estimate_from() {
    // Extrapolating from zero finished jobs is not a worse estimate, it is a
    // made-up one.
    let visible = strip_ansi(&track_line(full_colour(), "prerender", 0, 30, 0, None));
    assert!(
        !visible.contains('~'),
        "estimated from nothing: {visible:?}"
    );

    let visible = strip_ansi(&track_line(
        full_colour(),
        "prerender",
        7,
        30,
        0,
        Some(Duration::from_millis(400)),
    ));
    assert!(
        visible.contains("~400ms"),
        "expected an estimate in {visible:?}"
    );
}

#[test]
fn the_trail_deepens_towards_the_runner() {
    // Light where the run started, deepening towards the runner - which is the
    // direction the ramp was asked for, and the one thing about it a machine
    // can hold on to.
    //
    // Measured as luminance rather than as redness. Redness was the first
    // version of this assertion and it was a proxy that happened to hold while
    // the ramp was orange; the moment the palette was resampled from the logo
    // and turned violet-to-cyan, the proxy inverted while the property it stood
    // for was still true. Brightness is the property.
    // Measured on a finished track, where every cell is covered ground.
    let cells = track_cell_colours(&runner_track(full_colour(), 30, 30, 0));
    assert!(cells.len() >= 2, "the trail should span several cells");
    assert!(
        luminance(&cells[0]) > luminance(&cells[cells.len() - 1]),
        "the trail should deepen towards the runner: {cells:?}"
    );
}

/// The 24-bit parameters of every cell in a rendered track, in order.
///
/// Owned rather than borrowed so a caller can render and measure in one
/// expression; the alternative is a `let` binding at every call site whose only
/// job is to outlive the slices.
fn track_cell_colours(track: &str) -> Vec<String> {
    track
        .match_indices("38;2;")
        .map(|(index, _)| {
            track[index + 5..]
                .split('m')
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

fn luminance(code: &str) -> f64 {
    let channel = |value: Option<&str>| {
        value
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or_default()
    };
    let mut parts = code.split(';');
    let (red, green, blue) = (
        channel(parts.next()),
        channel(parts.next()),
        channel(parts.next()),
    );
    0.2126 * red + 0.7152 * green + 0.0722 * blue
}

#[test]
fn the_covered_ground_is_green_and_the_ground_ahead_is_not_coloured_at_all() {
    // Done is green, because that is what a progress bar has always meant by
    // green. The half the runner has not reached stays in the `dim` role: it is
    // the background the bar is measured against, and a ramp there competes
    // with the one that carries the measurement.
    let track = runner_track(full_colour(), 20, 30, 0);
    let covered = track_cell_colours(&track);
    assert!(covered.len() >= 2, "the trail should span several cells");
    for cell in &covered {
        let mut parts = cell
            .split(';')
            .map(|value| value.parse::<u16>().unwrap_or(0));
        let (red, green, blue) = (
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
        );
        assert!(
            green > red && green > blue,
            "a covered cell is not green: {cell}"
        );
    }

    // One 24-bit colour per covered cell and not one more, which is what proves
    // the cells after the runner were left alone.
    let (behind, _, _) = runner_cells(UNICODE_GLYPHS, 20, 30, 0);
    assert_eq!(covered.len(), behind.chars().count());
}

#[test]
fn an_untouched_track_carries_no_ramp_at_all() {
    // The frame in the report that started this: at 0% there is nothing done
    // yet, so there is nothing to colour.
    let track = runner_track(full_colour(), 0, 30, 0);
    assert!(
        track_cell_colours(&track).is_empty(),
        "an empty track should be drawn in the dim role only: {track:?}"
    );
}

#[test]
fn a_progress_frame_on_a_plain_terminal_carries_no_escapes_from_the_ramp() {
    let line = track_line(Capabilities::PLAIN, "prerender", 5, 10, 0, None);
    assert_eq!(strip_ansi(&line), line, "a plain terminal got escapes");
}

// ─── The spinner ─────────────────────────────────────────────────────────────

#[test]
fn a_spinner_frame_fits_the_narrowest_terminal() {
    for frame in 0..24 {
        let line = spinner_line(full_colour(), "bundling", frame, Duration::from_secs(90));
        assert!(columns(&line) <= NARROWEST_TERMINAL);
    }
}

#[test]
fn a_spinner_cycles_through_every_frame_of_its_glyph_set() {
    let frames = UNICODE_GLYPHS.spinner.len();
    let first = strip_ansi(&spinner_line(full_colour(), "bundling", 0, Duration::ZERO));
    let wrapped = strip_ansi(&spinner_line(
        full_colour(),
        "bundling",
        frames,
        Duration::ZERO,
    ));
    assert_eq!(first, wrapped, "the frame index should wrap, not overflow");
}

#[test]
fn a_counter_changes_role_as_a_phase_ages() {
    // Nothing is wrong at either threshold. A build stuck on one phase should
    // simply not look like a build working through several.
    let at = |seconds: u64| phase_line("bundling", "x".to_string(), Duration::from_secs(seconds));
    assert_ne!(at(1), at(9), "a slow phase should not read as a fast one");
    assert_ne!(at(9), at(60), "a stuck phase should not read as a slow one");
}

// ─── Bars ────────────────────────────────────────────────────────────────────

#[test]
fn painting_a_bar_changes_its_colour_and_not_its_length() {
    // The raw bar is what the column was measured with; a painted one that
    // gained or lost a cell would push the table's right edge out.
    let raw = bar(7.0, 10.0, 12);
    assert_eq!(strip_ansi(&heat_bar(&raw, 12)), raw);
}

#[test]
fn a_short_bar_stays_cool_and_only_a_full_one_runs_hot() {
    // Coloured against the full column rather than against its own length, so
    // the fastest row does not get a red tip.
    let cool = heat_bar(&bar(1.0, 100.0, 12), 12);
    let hot = heat_bar(&bar(100.0, 100.0, 12), 12);
    assert_ne!(cool, hot);
    assert!(
        strip_ansi(&cool).chars().count() < strip_ansi(&hot).chars().count(),
        "a one-cell bar should be shorter than a full one"
    );
}
