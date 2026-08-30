//! The one writer every durable line goes through.
//!
//! `println!` panics when the write fails, and the write fails as soon as the
//! reader goes away: `ruvyxa routes | head -5`, `ruvyxa check | grep -q RUV`,
//! `less` quit early, a CI log collector that closed its end. What the user
//! then sees is a panic message and a non-zero exit for a command that did
//! exactly what was asked of it — `grep -q` reports failure for the wrong
//! reason.
//!
//! So a closed pipe is a clean stop here and nothing else is. Swallowing every
//! write error would hide a full disk when stdout is redirected to a file,
//! which is a real failure and has to keep behaving the way it does today:
//! every error but [`io::ErrorKind::BrokenPipe`] still panics, with the message
//! `println!` itself would have produced.
//!
//! Every line also goes through [`crate::sanitize::sanitize_styled`] on the way
//! out. A role sanitizes the value it paints, but a caller composing its own
//! row may print a cell exactly as it received it — the route table's `path`
//! column is one — and this is the last place that text is still a string.
//!
//! The transient frames are not here. They are written to stderr with
//! `eprint!`, which is deliberate (see [`crate::progress`]) and already
//! tolerates a closed stream because its flush result is discarded.

use std::io::{self, Write};

use crate::sanitize::sanitize_styled;

/// Write `line` and a newline to stdout.
///
/// One line: the filter replaces a control character wherever it appears, so a
/// caller that wants a blank line above or below its text asks for one with
/// [`print_blank_line`] rather than writing `\n` into the string.
pub fn print_line(line: &str) {
    write_out(&sanitize_styled(line), true);
}

/// Write `fragment` to stdout with no newline, for a caller composing one line
/// from several pieces.
pub fn print_fragment(fragment: &str) {
    write_out(&sanitize_styled(fragment), false);
}

/// Write an empty line.
pub fn print_blank_line() {
    write_out("", true);
}

fn write_out(text: &str, newline: bool) {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    let result = if newline {
        writeln!(handle, "{text}")
    } else {
        write!(handle, "{text}")
    };
    report(result);
}

/// What a failed write to stdout means.
///
/// Split out from the write so the decision can be asserted without a pipe: a
/// broken pipe is the reader leaving, and a test cannot arrange one portably.
pub(crate) fn report(result: io::Result<()>) {
    match result {
        Ok(()) => {}
        // The reader has gone. Nothing is wrong with this process, and there is
        // nowhere left to report anything to.
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {}
        // A full disk, a revoked handle: the same panic `println!` raises, so
        // the one behaviour that changes here is the pipe.
        Err(error) => panic!("failed printing to stdout: {error}"),
    }
}
