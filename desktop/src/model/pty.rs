//! What a shell has written, kept by the window rather than by the pane.
//!
//! The kernel broadcasts a terminal's output to every client attached to the
//! place, so the window's own connection already carries it. It arrives
//! before there is a pane to draw it in — the drawer opens on the `board`
//! frame and the pane is built on the frame after that — and it goes on
//! arriving while the tab sits behind another one. Held here, none of it is
//! missed: the pane is seeded with the shell's scrollback the moment it
//! mounts, prompt included.
//!
//! Before this, a pane opened a second attach socket of its own and asked
//! that one for the output. The prompt had already gone out over the
//! connection the window was holding, so the new socket heard nothing and
//! the drawer showed an empty screen until the person typed.

use std::collections::HashMap;

/// How much of one shell's output is kept: enough scrollback to seed a pane
/// with, not a full history. The emulator keeps its own once it has been fed.
const CAP: usize = 128 * 1024;

/// One shell's output, as the window holds it.
#[derive(Debug, Default)]
pub struct PtyStream {
    /// The tail of what the shell has written.
    tail: Vec<u8>,
    /// How many bytes have fallen off the front of `tail`. A reader's cursor
    /// counts from the shell's first byte, so this is what places it.
    dropped: u64,
    /// The chat whose socket these bytes are taken from.
    ///
    /// Every chat in a place holds its own attach connection and the kernel
    /// broadcasts to all of them, so the same output arrives once per chat.
    /// The first to deliver a page claims it and the rest are ignored; the
    /// claim is dropped when that chat's socket goes, so another can take
    /// over. Reading every copy would print the shell's output twice.
    reader: Option<u64>,
}

impl PtyStream {
    /// How much the shell has written in all. A fresh reader starts here to
    /// take only what comes next, or at 0 to take the scrollback with it.
    pub fn written(&self) -> u64 {
        self.dropped + self.tail.len() as u64
    }

    /// Everything since `cursor`, and where that leaves it. A cursor from
    /// before what is still held resumes at the oldest byte there is: the
    /// reader missed some, which is what the cap means.
    pub fn since(&self, cursor: u64) -> (&[u8], u64) {
        let from = (cursor.max(self.dropped) - self.dropped) as usize;
        let from = from.min(self.tail.len());
        (&self.tail[from..], self.written())
    }

    fn append(&mut self, bytes: &[u8]) {
        self.tail.extend_from_slice(bytes);
        if self.tail.len() > CAP {
            // Trimmed on a byte, not on an escape sequence: what is cut is
            // scrollback nobody has looked at in 128 KB of output, and the
            // pane that reads it next is fed the rest of the stream, in
            // which the shell redraws its own screen.
            let cut = self.tail.len() - CAP;
            self.tail.drain(..cut);
            self.dropped += cut as u64;
        }
    }
}

/// Every shell of one project, by the kernel's id for it (`t1`).
#[derive(Debug, Default)]
pub struct PtyStreams(HashMap<String, PtyStream>);

impl PtyStreams {
    /// Take output for `page` from `chat`, unless another live chat is
    /// already the one being read. Answers whether the bytes were kept.
    ///
    /// `live` says whether a chat still holds a socket; a claim by one that
    /// has gone is not a claim any more.
    pub fn accept(
        &mut self,
        chat: u64,
        page: &str,
        bytes: &[u8],
        live: impl Fn(u64) -> bool,
    ) -> bool {
        let stream = self.0.entry(page.to_owned()).or_default();
        match stream.reader {
            Some(held) if held != chat && live(held) => return false,
            _ => stream.reader = Some(chat),
        }
        stream.append(bytes);
        true
    }

    pub fn get(&self, page: &str) -> Option<&PtyStream> {
        self.0.get(page)
    }

    /// Forget the shells that are no longer open, so a closed terminal's
    /// scrollback is not carried for the life of the window.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.0.retain(|page, _| keep(page));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pane_that_mounts_late_is_still_given_the_prompt() {
        let mut streams = PtyStreams::default();
        assert!(streams.accept(1, "t1", b"$ ", |_| true));
        let (bytes, cursor) = streams.get("t1").unwrap().since(0);
        assert_eq!(
            bytes, b"$ ",
            "the prompt was written before the pane existed"
        );
        assert_eq!(cursor, 2);
        assert!(streams.accept(1, "t1", b"ls\r\n", |_| true));
        let (bytes, cursor) = streams.get("t1").unwrap().since(cursor);
        assert_eq!(bytes, b"ls\r\n", "and only what came after it");
        assert_eq!(cursor, 6);
    }

    #[test]
    fn one_chats_copy_is_read_and_the_others_are_not() {
        let mut streams = PtyStreams::default();
        assert!(streams.accept(1, "t1", b"hello", |_| true));
        assert!(
            !streams.accept(2, "t1", b"hello", |_| true),
            "every chat in the place is sent the same output"
        );
        assert_eq!(streams.get("t1").unwrap().since(0).0, b"hello");
    }

    #[test]
    fn a_reader_whose_socket_went_hands_the_page_on() {
        let mut streams = PtyStreams::default();
        assert!(streams.accept(1, "t1", b"a", |_| true));
        assert!(streams.accept(2, "t1", b"b", |chat| chat != 1));
        assert_eq!(streams.get("t1").unwrap().since(0).0, b"ab");
    }

    #[test]
    fn past_the_cap_the_oldest_goes_and_the_cursor_says_so() {
        let mut streams = PtyStreams::default();
        streams.accept(1, "t1", &vec![b'x'; CAP + 16], |_| true);
        let stream = streams.get("t1").unwrap();
        assert_eq!(stream.written(), CAP as u64 + 16);
        let (bytes, cursor) = stream.since(0);
        assert_eq!(bytes.len(), CAP, "only the tail is kept");
        assert_eq!(cursor, CAP as u64 + 16);
    }
}
