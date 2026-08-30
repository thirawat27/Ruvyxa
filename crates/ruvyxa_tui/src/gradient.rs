//! Decorative colour: ramps that carry no meaning and may therefore be pretty.
//!
//! [`crate::theme`] owns the roles, and a role stays inside the 16-colour range
//! because it is the only carrier of a distinction — `ok` against `warn`, a
//! page route against an API route. Nothing in this file carries a
//! distinction. A wordmark is the same wordmark in one colour; the trail behind
//! the progress runner says nothing the fraction beside it does not already
//! say; a rule that fades out is a rule. That is exactly what makes 24-bit
//! colour safe here and unsafe there.
//!
//! Every [`Gradient`] therefore names a **fallback role code**, and a terminal
//! that reports fewer colours gets that one flat code instead of an
//! approximation. The picture degrades; nothing goes missing.
//!
//! # Two ways to walk a ramp
//!
//! [`Gradient::paint`] walks it once from left to right — for a wordmark, a
//! rule, a bar, anything whose ends mean "start" and "end".
//!
//! [`Gradient::paint_cycled`] wraps the last stop back around to the first and
//! offsets the whole ramp by a phase, so repainting the same text with a
//! rising phase makes light appear to travel through it. That is the shimmer on
//! the spinner and on the runner's trail, and it is why those two look alive
//! while a static frame stays legible: at any single phase the frame is still
//! just coloured text.
//!
//! Spaces are left unpainted. A space with a foreground colour looks identical
//! to one without, and skipping it is a third of the escape bytes on a line
//! that is repainted ten times a second.

use crate::sanitize::sanitize_plain;
use crate::theme::{ColorDepth, HEADING_CODE, color_depth, paint_when};

/// A 24-bit colour. Kept as a plain tuple struct so a ramp can be written as a
/// literal table and read as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// A named decorative ramp: the stops, and the single role code a terminal
/// without enough colours gets instead.
#[derive(Debug, Clone, Copy)]
pub struct Gradient {
    pub stops: &'static [Rgb],
    /// The 16-colour role this collapses to. Never empty — a gradient with
    /// nothing to fall back to would simply vanish on a 16-colour terminal.
    pub fallback: &'static str,
    pub bold: bool,
}

/// The Ruvyxa wordmark, in the mark's own colours.
///
/// These five stops are sampled from `assets/branding/ruvyxa.png` rather than
/// picked by eye: the pixels of the logo were bucketed along the diagonal the
/// mark's gradient runs on, and the bucket averages are what is written here.
/// That is why the palette is magenta through to cyan and not the warm ramp it
/// started as — a terminal wordmark that does not match the logo beside it in
/// the README is not brand colour, it is a second brand.
///
/// Falls back to `heading`'s bold magenta, which is both the colour the header
/// had before the ramp existed and the colour the mark itself starts on, so a
/// sixteen-colour terminal loses the ramp and keeps the identity.
pub const BRAND: Gradient = Gradient {
    stops: &[
        Rgb(251, 101, 251),
        Rgb(176, 52, 251),
        Rgb(95, 55, 247),
        Rgb(28, 128, 248),
        Rgb(30, 223, 252),
    ],
    fallback: HEADING_CODE,
    bold: true,
};

/// The ground the progress runner has already covered: light green where it
/// started, deepening towards the runner.
///
/// Green rather than brand colour because this half means *done*, and every
/// progress bar a developer has ever read says done in green. It was drawn in
/// the mark's violet-to-cyan for exactly one release and read as decoration
/// rather than as a measurement.
///
/// The ramp runs light to deep in reading order, so the oldest ground is the
/// palest. Reversing it is a one-line change here and nowhere else.
pub const TRAIL: Gradient = Gradient {
    stops: &[Rgb(160, 248, 184), Rgb(74, 208, 126), Rgb(26, 122, 78)],
    fallback: "32",
    bold: false,
};

/// A rule that starts on the mark's magenta and fades into the surrounding
/// text. Used under a command header and after a section title, where a flat
/// line of dashes reads as a border and a fading one reads as an underline.
pub const RULE: Gradient = Gradient {
    stops: &[Rgb(226, 84, 250), Rgb(110, 92, 210), Rgb(72, 78, 96)],
    fallback: "90",
    bold: false,
};

/// Magnitude: green for the cheap end of a table, red for the expensive one.
///
/// The one ramp deliberately left off the brand palette. Its colour is *nearly*
/// meaning — a reader takes green as fast and red as slow before reading the
/// number — and recolouring it to magenta-and-cyan would trade a reading
/// everybody already has for a matching swatch. It is still only ever drawn
/// beside the number it summarises, never instead of it.
pub const HEAT: Gradient = Gradient {
    stops: &[Rgb(88, 214, 141), Rgb(240, 200, 88), Rgb(240, 96, 96)],
    fallback: "95",
    bold: false,
};

/// The travelling highlight on a spinner: the mark's violet and cyan with a
/// near-white crest between them. Cycled, so its ends must meet — the first and
/// last stops are the same colour on purpose.
pub const PULSE: Gradient = Gradient {
    stops: &[
        Rgb(120, 70, 230),
        Rgb(40, 190, 250),
        Rgb(224, 250, 255),
        Rgb(40, 190, 250),
        Rgb(120, 70, 230),
    ],
    fallback: "36",
    bold: false,
};

impl Gradient {
    /// Walks the ramp once across `text`, resolved against the process's
    /// detected depth.
    pub fn paint(&self, text: impl AsRef<str>) -> String {
        self.paint_with(color_depth(), text)
    }

    /// The pure half of [`Gradient::paint`]: depth decided by the caller.
    pub fn paint_with(&self, depth: ColorDepth, text: impl AsRef<str>) -> String {
        self.render(depth, text.as_ref(), None)
    }

    /// Walks the ramp cyclically, offset by `phase` turns. Repainting the same
    /// text with a rising phase moves the highlight along it.
    pub fn paint_cycled(&self, text: impl AsRef<str>, phase: f64) -> String {
        self.paint_cycled_with(color_depth(), text, phase)
    }

    pub fn paint_cycled_with(
        &self,
        depth: ColorDepth,
        text: impl AsRef<str>,
        phase: f64,
    ) -> String {
        self.render(depth, text.as_ref(), Some(phase))
    }

    /// One cell painted at a fixed point along the ramp, for a caller that is
    /// composing its own row of cells and knows where each one sits.
    pub fn cell(&self, depth: ColorDepth, text: &str, position: f64) -> String {
        match self.code(depth, position) {
            // Filtered for the same reason `paint_when` filters, and it has to
            // be spelled again because this branch writes its own escape rather
            // than going through the role.
            Some(code) => format!("\x1b[{code}m{}\x1b[0m", sanitize_plain(text)),
            None => paint_when(depth != ColorDepth::None, text, self.fallback),
        }
    }

    /// The colour at `position` in `0.0..=1.0`, clamped.
    pub fn sample(&self, position: f64) -> Rgb {
        sample(self.stops, position.clamp(0.0, 1.0))
    }

    fn render(&self, depth: ColorDepth, text: &str, phase: Option<f64>) -> String {
        if depth < ColorDepth::Ansi256 {
            // Ansi16 and None both resolve through the role, which already
            // knows how to emit nothing when colour is off — and sanitizes.
            return paint_when(depth != ColorDepth::None, text, self.fallback);
        }

        // A ramp walks the text one character at a time and gives each one a
        // colour, so an escape sequence smuggled in would be coloured character
        // by character and emitted in pieces. There is no shape of styled input
        // this can render, which is why the filter here is the plain one.
        let text = &sanitize_plain(text);
        let total = text.chars().count();
        if total == 0 {
            return String::new();
        }

        // Two escapes per visible character in the worst case, plus one reset.
        let mut painted = String::with_capacity(text.len() + total * 20 + 4);
        let mut last: Option<String> = None;
        for (index, character) in text.chars().enumerate() {
            if character == ' ' {
                // A coloured space is an invisible space. Close the run so the
                // gap does not inherit a colour it never shows.
                if last.take().is_some() {
                    painted.push_str("\x1b[0m");
                }
                painted.push(character);
                continue;
            }

            let along = position_of(index, total);
            let position = match phase {
                Some(phase) => (along + phase).rem_euclid(1.0),
                None => along,
            };
            let code = self
                .code(depth, position)
                .expect("depth checked to be at least Ansi256");
            if last.as_deref() != Some(code.as_str()) {
                painted.push_str("\x1b[");
                painted.push_str(&code);
                painted.push('m');
                last = Some(code);
            }
            painted.push(character);
        }
        if last.is_some() {
            painted.push_str("\x1b[0m");
        }
        painted
    }

    /// SGR parameters for one point on the ramp, or `None` when the depth
    /// cannot express a ramp at all and the fallback role must be used.
    fn code(&self, depth: ColorDepth, position: f64) -> Option<String> {
        let Rgb(red, green, blue) = self.sample(position);
        let weight = if self.bold { "1;" } else { "" };
        match depth {
            ColorDepth::TrueColor => Some(format!("{weight}38;2;{red};{green};{blue}")),
            ColorDepth::Ansi256 => Some(format!(
                "{weight}38;5;{}",
                to_ansi256(Rgb(red, green, blue))
            )),
            _ => None,
        }
    }
}

/// Where character `index` of `total` sits along the ramp.
///
/// A single character sits at the start rather than dividing by zero, and the
/// last character of a longer run lands exactly on `1.0` so a ramp reaches its
/// final stop instead of stopping one step short.
fn position_of(index: usize, total: usize) -> f64 {
    if total <= 1 {
        return 0.0;
    }
    index as f64 / (total - 1) as f64
}

/// Linear interpolation across a stop table.
fn sample(stops: &[Rgb], position: f64) -> Rgb {
    match stops {
        [] => Rgb(255, 255, 255),
        [only] => *only,
        _ => {
            let segments = stops.len() - 1;
            let scaled = position * segments as f64;
            // `min` rather than a clamp on `scaled`: at position 1.0 the index
            // is exactly `segments`, which has no stop after it to interpolate
            // towards.
            let index = (scaled.floor() as usize).min(segments - 1);
            mix(stops[index], stops[index + 1], scaled - index as f64)
        }
    }
}

fn mix(from: Rgb, to: Rgb, amount: f64) -> Rgb {
    let channel = |from: u8, to: u8| {
        (f64::from(from) + (f64::from(to) - f64::from(from)) * amount).round() as u8
    };
    Rgb(
        channel(from.0, to.0),
        channel(from.1, to.1),
        channel(from.2, to.2),
    )
}

/// The nearest entry in the xterm-256 palette: the 6×6×6 colour cube, or the
/// 24-step grey ramp when all three channels agree.
///
/// The grey branch is not an optimisation. Quantising a neutral through the
/// cube snaps it to one of six levels, which is what turns a fading rule into
/// three visible steps instead of a fade.
pub fn to_ansi256(Rgb(red, green, blue): Rgb) -> u8 {
    if red == green && green == blue {
        if red < 8 {
            return 16;
        }
        if red > 248 {
            return 231;
        }
        return 232 + ((u16::from(red) - 8) * 24 / 247) as u8;
    }

    let level = |channel: u8| u16::from(channel) * 5 / 255;
    (16 + 36 * level(red) + 6 * level(green) + level(blue)) as u8
}
