//! Composing a document around HTML that is still being produced.
//!
//! A buffered render hands the host one string, and [`compose_localized_document`]
//! puts the asset links into its `<head>` and the hydration script before its
//! `</body>`. A streamed render never produces that string: React emits the
//! shell as soon as it is ready and each `Suspense` boundary as it resolves, so
//! the host is forwarding bytes long before it knows what the last of them are.
//!
//! This is the same composition, done to a stream. Two edits, at the two ends:
//!
//! - The head is injected once `</head>` has been seen. Chunks are held until
//!   then, which costs nothing in practice — React writes the whole head region
//!   into the shell — and is what keeps the stylesheet ahead of the content it
//!   styles. A document that never closes its head is emitted unedited rather
//!   than held forever.
//! - The tail is injected at the end, before `</body>`. A rolling window of the
//!   last bytes is held back for exactly this: the closing tags are the last
//!   thing React writes, and holding a fixed window is what lets them be found
//!   without buffering the document.
//!
//! The Flight payload belongs to the tail and arrives with the frame that ends
//! the stream, which is why the tail is a closure rather than a string: it is
//! built when the body is over, from something that did not exist when it began.
//!
//! Nothing here caches. A streamed document is produced per request by
//! definition — a route whose document can be stored has to become a string, and
//! that route does not come through here.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::Bytes;
use futures_util::Stream;

/// Bytes held back from the end of the stream so the closing tags can be found.
///
/// React writes `</body></html>` last, sometimes behind the inline scripts that
/// resolve late `Suspense` boundaries. A window this size holds all of that with
/// room to spare, and costs one small buffer for the life of the response.
const TAIL_WINDOW: usize = 1024;

/// Largest prefix held while waiting for `</head>`.
///
/// A bound rather than a promise: a document whose head is larger than this, or
/// which has no head at all, is forwarded unedited instead of being held. The
/// page then loads without the injected stylesheet, which is visibly wrong and
/// therefore reportable — where holding forever would look like a hung server.
const MAX_HEAD_PREFIX: usize = 512 * 1024;

/// A document stream with the head and tail this host is responsible for.
pub(crate) struct StreamedDocument<S, T> {
    inner: S,
    /// `None` once the head has been dealt with, one way or another.
    head: Option<HeadInjection>,
    /// Bytes seen but not yet emitted: the head prefix, then the tail window.
    held: Vec<u8>,
    /// How much of `held` has already been searched for `</head>`.
    ///
    /// Without it each chunk rescanned the whole prefix, so holding a large head
    /// cost O(prefix^2 / chunk) on the request path. The next scan resumes six
    /// bytes early -- `"</head>".len() - 1` -- because the needle may straddle
    /// the boundary between what was scanned and what just arrived.
    scanned: usize,
    tail: Option<T>,
    finished: bool,
}

/// What to do with the document's head once it can be found.
struct HeadInjection {
    /// Applied to the prefix through `</head>`; returns the edited prefix.
    compose: Box<dyn FnOnce(&str) -> String + Send>,
}

impl<S, T> StreamedDocument<S, T> {
    /// Wrap `inner`, composing the head with `compose` and ending with `tail`.
    ///
    /// `tail` is called once, after the body has finished, so it can read a
    /// value the stream itself delivered — the Flight payload above all.
    pub(crate) fn new(
        inner: S,
        compose: impl FnOnce(&str) -> String + Send + 'static,
        tail: T,
    ) -> Self {
        Self {
            inner,
            head: Some(HeadInjection {
                compose: Box::new(compose),
            }),
            held: Vec::new(),
            scanned: 0,
            tail: Some(tail),
            finished: false,
        }
    }
}

impl<S, T> StreamedDocument<S, T>
where
    T: FnOnce() -> String,
{
    /// Emit the head prefix if `</head>` is now in the held bytes.
    ///
    /// Also emits, unedited, when the bound is reached: a document this cannot
    /// find a head in is still a document, and forwarding it beats holding it.
    fn take_head(&mut self) -> Option<Bytes> {
        let injection = self.head.as_ref()?;
        let _ = injection;
        const NEEDLE: &[u8] = b"</head>";
        let from = self.scanned.saturating_sub(NEEDLE.len() - 1);
        let end = find_ascii_case(&self.held[from..], NEEDLE).map(|at| from + at + NEEDLE.len());
        let split = match end {
            Some(split) => split,
            None if self.held.len() >= MAX_HEAD_PREFIX => self.held.len(),
            None => {
                self.scanned = self.held.len();
                return None;
            }
        };
        // Only on a character boundary: a chunk may cut a multi-byte sequence in
        // half, and composing over half of one would corrupt it.
        let split = floor_char_boundary(&self.held, split);
        let head = self.head.take()?;
        let prefix = String::from_utf8_lossy(&self.held[..split]).into_owned();
        let rest = self.held.split_off(split);
        self.held = rest;
        self.scanned = 0;
        let composed = if end.is_some() {
            (head.compose)(&prefix)
        } else {
            prefix
        };
        Some(Bytes::from(composed.into_bytes()))
    }

    /// Emit everything past the tail window, keeping the window itself held.
    fn take_body(&mut self) -> Option<Bytes> {
        if self.head.is_some() || self.held.len() <= TAIL_WINDOW {
            return None;
        }
        let split = floor_char_boundary(&self.held, self.held.len() - TAIL_WINDOW);
        if split == 0 {
            return None;
        }
        let rest = self.held.split_off(split);
        let emitted = std::mem::replace(&mut self.held, rest);
        Some(Bytes::from(emitted))
    }

    /// Emit everything left, with the tail inserted before `</body>`.
    fn take_end(&mut self) -> Bytes {
        let tail = self.tail.take().map(|build| build()).unwrap_or_default();
        // The head never arrived: emit the prefix as it stands rather than lose
        // it. `take_head` has already given up on editing by this point.
        self.head = None;
        let mut held = String::from_utf8_lossy(&std::mem::take(&mut self.held)).into_owned();
        match find_ascii_case(held.as_bytes(), b"</body>") {
            Some(at) => held.insert_str(at, &tail),
            // No closing tag to sit before. Appending still reaches the browser:
            // the parser moves a trailing script into the body it already has.
            None => held.push_str(&tail),
        }
        Bytes::from(held.into_bytes())
    }
}

impl<S, T> Stream for StreamedDocument<S, T>
where
    S: Stream<Item = std::result::Result<Bytes, io::Error>> + Unpin,
    T: FnOnce() -> String + Unpin,
{
    type Item = std::result::Result<Bytes, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if self.finished {
                return Poll::Ready(None);
            }
            if let Some(chunk) = self.take_head() {
                return Poll::Ready(Some(Ok(chunk)));
            }
            if let Some(chunk) = self.take_body() {
                return Poll::Ready(Some(Ok(chunk)));
            }
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.held.extend_from_slice(&chunk);
                    // Round again rather than emit: the new bytes may be what
                    // completes the head, and the head must go out first.
                }
                Poll::Ready(Some(Err(error))) => {
                    self.finished = true;
                    return Poll::Ready(Some(Err(error)));
                }
                Poll::Ready(None) => {
                    self.finished = true;
                    return Poll::Ready(Some(Ok(self.take_end())));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Index of `needle` in `haystack`, comparing ASCII letters case-insensitively.
fn find_ascii_case(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
}

/// The largest index at or below `at` that does not split a UTF-8 sequence.
fn floor_char_boundary(bytes: &[u8], at: usize) -> usize {
    let mut split = at.min(bytes.len());
    // `bytes[split]` would be out of range at the end, and the end is always a
    // boundary anyway: there is no continuation byte after the last one.
    while split > 0 && split < bytes.len() && (bytes[split] & 0b1100_0000) == 0b1000_0000 {
        split -= 1;
    }
    split
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    fn chunks(parts: &[&str]) -> impl Stream<Item = std::result::Result<Bytes, io::Error>> + Unpin {
        futures_util::stream::iter(
            parts
                .iter()
                .map(|part| Ok(Bytes::from(part.as_bytes().to_vec())))
                .collect::<Vec<_>>(),
        )
    }

    async fn collect<S, T>(document: StreamedDocument<S, T>) -> String
    where
        S: Stream<Item = std::result::Result<Bytes, io::Error>> + Unpin,
        T: FnOnce() -> String + Unpin,
    {
        let mut out = Vec::new();
        let mut document = document;
        while let Some(chunk) = document.next().await {
            out.extend_from_slice(&chunk.unwrap());
        }
        String::from_utf8(out).unwrap()
    }

    /// `take_head` resumes its search instead of restarting it, so the needle
    /// can land across the boundary between one scan and the next. Every split
    /// of `</head>` is exercised, because an off-by-one in the resume overlap
    /// loses exactly one of them and nothing else would notice.
    #[tokio::test]
    async fn finds_a_head_split_across_two_chunks_at_every_offset() {
        const NEEDLE: &str = "</head>";
        for split in 0..=NEEDLE.len() {
            let first = format!("<html><head><title>x</title>{}", &NEEDLE[..split]);
            let second = format!("{}<body><p>hi</p></body></html>", &NEEDLE[split..]);
            let parts: [&str; 2] = [&first, &second];
            let document = StreamedDocument::new(
                chunks(&parts),
                |prefix| prefix.replace("</head>", "<link rel=stylesheet></head>"),
                String::new,
            );
            assert_eq!(
                collect(document).await,
                "<html><head><title>x</title><link rel=stylesheet></head><body><p>hi</p></body></html>",
                "`</head>` split after {split} byte(s) was not composed",
            );
        }
    }

    #[tokio::test]
    async fn injects_the_head_and_the_tail_at_the_two_ends() {
        let document = StreamedDocument::new(
            chunks(&[
                "<html><head><title>x</title>",
                "</head><body><p>hi",
                "</p></body></html>",
            ]),
            |prefix| prefix.replace("</head>", "<link rel=stylesheet></head>"),
            || "<script src=/c.js></script>".to_string(),
        );
        assert_eq!(
            collect(document).await,
            "<html><head><title>x</title><link rel=stylesheet></head><body><p>hi</p><script src=/c.js></script></body></html>",
        );
    }

    /// The point of the whole file: the shell has to leave before the rest of
    /// the document exists, or a slow boundary delays the first paint.
    ///
    /// The source below yields the shell, then `Pending`, then the rest. A
    /// composer that buffered the document would answer nothing until the very
    /// end; this one answers the shell while the source is still pending.
    #[tokio::test]
    async fn emits_the_shell_before_the_stream_has_finished() {
        let mut document = StreamedDocument::new(
            PausingStream {
                remaining: vec!["</body></html>", "<html><head></head><body>shell"],
                paused: false,
            },
            |prefix| prefix.replace("</head>", "<style></style></head>"),
            || "<!--end-->".to_string(),
        );

        let first = document.next().await.unwrap().unwrap();
        assert_eq!(&first[..], b"<html><head><style></style></head>");

        let mut rest = Vec::new();
        while let Some(chunk) = document.next().await {
            rest.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(
            String::from_utf8(rest).unwrap(),
            "<body>shell<!--end--></body></html>"
        );
    }

    /// Yields one chunk, then `Pending` once, then the next — a source that has
    /// not finished producing when the shell is already sendable.
    struct PausingStream {
        remaining: Vec<&'static str>,
        paused: bool,
    }

    impl Stream for PausingStream {
        type Item = std::result::Result<Bytes, io::Error>;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            if !self.paused && self.remaining.len() == 1 {
                self.paused = true;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            match self.remaining.pop() {
                Some(part) => Poll::Ready(Some(Ok(Bytes::from(part.as_bytes().to_vec())))),
                None => Poll::Ready(None),
            }
        }
    }

    /// A head this never finds must not hold the response open.
    #[tokio::test]
    async fn forwards_a_document_with_no_head_unedited() {
        let document = StreamedDocument::new(
            chunks(&["<p>fragment</p>"]),
            |prefix| format!("EDITED{prefix}"),
            || "<!--tail-->".to_string(),
        );
        assert_eq!(collect(document).await, "<p>fragment</p><!--tail-->");
    }

    /// A chunk boundary is not a character boundary, and composing over half of
    /// a multi-byte sequence would corrupt it.
    #[tokio::test]
    async fn never_splits_a_multi_byte_character() {
        let text = "สวัสดี".repeat(400);
        let parts = ["<html><head></head><body>", text.as_str(), "</body></html>"];
        let document =
            StreamedDocument::new(chunks(&parts), |prefix| prefix.to_string(), String::new);
        assert_eq!(
            collect(document).await,
            format!("<html><head></head><body>{text}</body></html>"),
        );
    }

    #[tokio::test]
    async fn passes_an_error_through_rather_than_swallowing_it() {
        let failing = futures_util::stream::iter(vec![
            Ok(Bytes::from_static(b"<html><head></head><body>")),
            Err(io::Error::other("worker died")),
        ]);
        let mut document = StreamedDocument::new(failing, |p| p.to_string(), String::new);
        assert!(document.next().await.unwrap().is_ok());
        assert!(document.next().await.unwrap().is_err());
        assert!(document.next().await.is_none());
    }
}
