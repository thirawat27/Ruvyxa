//! What a value is allowed to be once it is on its way to a terminal.
//!
//! A terminal is an interpreter, and the text this crate prints is not all
//! ours. Repository file paths reach it directly — `ruvyxa routes` prints the
//! path of every discovered route, `ruvyxa check` and `ruvyxa build` print the
//! project root and the file behind every diagnostic — and a path is whatever
//! the author of the repository named a file.
//!
//! The realistic vector is continuous integration. A pull request from a fork
//! runs `ruvyxa check` against the fork's tree; a file named with `\x1b[2J`, an
//! OSC title-set sequence, or a run of newlines and forged `✓` glyphs can then
//! rewrite what a human reviewer sees in the log — hiding a failure, or forging
//! a pass. Locally the bar is lower, because the developer is about to build
//! that repository anyway, but the log is read by somebody who is not.
//!
//! # Two filters, because two kinds of text arrive here
//!
//! [`sanitize_plain`] is for a **value**: a path, a label, a count, a name.
//! Nothing that carries meaning in this crate is written with escapes, so a
//! value has no legitimate control character in it at all and every one is
//! replaced. It runs inside [`crate::theme::paint_when`] and inside
//! [`crate::gradient::Gradient`], which is what makes it cover `path_text`,
//! every role, and every ramp at once — and it runs whether or not colour is
//! on, because a redirected CI log is exactly the case where it is off.
//!
//! [`sanitize_styled`] is for a **finished line**, which by then legitimately
//! carries the escapes this crate just added. It keeps a colour change —
//! `ESC [ … m`, and only that — and replaces every other escape and every other
//! control character. It runs in [`crate::stream`], so a line assembled from
//! parts that never went through a role (a table cell printed as it came, for
//! instance) is still filtered on the way out.
//!
//! A tab survives both. It is the one control character that means "layout"
//! rather than "command", and a cell or a duration may hold one.

/// What a rejected control character is replaced with.
///
/// U+FFFD rather than deletion: a name that was tampered with should look
/// tampered with, and a silently shortened path reads as an ordinary one.
pub const REPLACEMENT: char = '\u{FFFD}';

/// Whether one character may not be written to a terminal as part of a value.
///
/// `char::is_control` covers C0, DEL, and the C1 range — 0x9B is a single-byte
/// CSI on a terminal that decodes it, so a filter that stopped at 0x7F would
/// leave a second way to open an escape sequence.
fn is_forbidden(character: char) -> bool {
    character.is_control() && character != '\t'
}

/// A value with every control character replaced.
///
/// Allocation-free when there is nothing to replace, which is every ordinary
/// path and every label the CLI prints.
#[must_use]
pub fn sanitize_plain(value: &str) -> String {
    if !value.chars().any(is_forbidden) {
        return value.to_string();
    }
    value
        .chars()
        .map(|character| {
            if is_forbidden(character) {
                REPLACEMENT
            } else {
                character
            }
        })
        .collect()
}

/// A finished line with every escape but a colour change replaced.
///
/// The escapes this crate emits are all `ESC [ params m`: `paint_when` writes
/// one, [`crate::gradient::Gradient`] writes one per colour run, and the
/// transient frames — which erase with `ESC [ 2 K` — go to stderr and never
/// reach here. So the allowlist is exactly SGR, and anything else a value
/// smuggled in loses its `ESC` and prints as the literal text it was.
#[must_use]
pub fn sanitize_styled(line: &str) -> String {
    if !line.chars().any(is_forbidden) {
        return line.to_string();
    }

    let characters = line.chars().collect::<Vec<_>>();
    let mut sanitized = String::with_capacity(line.len());
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if character == '\u{1b}' {
            match select_graphic_rendition(&characters[index..]) {
                Some(length) => {
                    sanitized.extend(&characters[index..index + length]);
                    index += length;
                }
                None => {
                    sanitized.push(REPLACEMENT);
                    index += 1;
                }
            }
            continue;
        }
        sanitized.push(if is_forbidden(character) {
            REPLACEMENT
        } else {
            character
        });
        index += 1;
    }
    sanitized
}

/// How long the `ESC [ params m` sequence at the front of `characters` is, or
/// `None` when what is there is any other escape.
///
/// The parameter bytes are the ones ECMA-48 allows in a CSI parameter string —
/// digits, `;`, and the `:` that separates the components of a 24-bit colour in
/// the form some terminals accept — so a sequence with a private-use
/// introducer, an intermediate byte, or any other final byte is not one of ours.
fn select_graphic_rendition(characters: &[char]) -> Option<usize> {
    if characters.first() != Some(&'\u{1b}') || characters.get(1) != Some(&'[') {
        return None;
    }
    let mut index = 2;
    while let Some(character) = characters.get(index) {
        match character {
            '0'..='9' | ';' | ':' => index += 1,
            'm' => return Some(index + 1),
            _ => return None,
        }
    }
    None
}
