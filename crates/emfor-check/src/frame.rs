//! Reading one complete message from a stream protocol.
//!
//! Each protocol supplies a [`Framer`]: given the bytes read so far, it
//! says how many bytes the message needs, that the message is complete,
//! or that it is not a valid message. [`FrameReader`] does the rest in one
//! place: it caps the message size, reads no more than it must, and tells
//! a close before the first byte from a close inside a message.
//!
//! A framer adds lengths with saturating (or checked) arithmetic, so an
//! overflow becomes `usize::MAX`; the reader turns any length above its
//! cap into [`FrameError::TooLarge`].
//!
//! A read error passes through as [`FrameError::Io`]; callers that count
//! `UnexpectedEof` (how TLS reports a close without close_notify) as a
//! close match it next to [`FrameError::Closed`].

use std::io;
use tokio::io::{AsyncRead, AsyncReadExt};

/// Bytes read at most per read.
const CHUNK: usize = 4096;

/// What a framer makes of the bytes read so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// The message is incomplete and needs at least this many bytes in
    /// all (`usize::MAX` when its length overflowed).
    Need(usize),
    /// The first this many bytes are one whole message.
    Complete(usize),
    /// Drop this many leading bytes that belong to no message (such as
    /// keepalives before it) and look again.
    Skip(usize),
    /// Not a valid message; the `down` reason.
    Invalid(String),
}

/// Finds the end of one message.
///
/// The reader calls [`Framer::frame`] again only after bytes were added
/// (or skipped), always with every byte of the message read so far, so a
/// framer may keep how far it has looked and continue from there.
pub trait Framer {
    fn frame(&mut self, buf: &[u8]) -> Frame;
}

impl<F: FnMut(&[u8]) -> Frame> Framer for F {
    fn frame(&mut self, buf: &[u8]) -> Frame {
        self(buf)
    }
}

/// Why no message was read.
#[derive(Debug)]
pub enum FrameError {
    /// The peer closed the connection (a read returned 0); `partial` when
    /// some bytes of the message had arrived.
    Closed {
        // Every protocol gives one reason for both today; kept so a reason
        // can tell them apart without touching the reader.
        #[allow(dead_code)]
        partial: bool,
    },
    /// The message is longer than the reader's cap.
    TooLarge,
    /// The framer's reason.
    Invalid(String),
    /// A read failed.
    Io(io::Error),
}

/// Reads whole messages, keeping bytes of the next one that arrived early.
#[derive(Debug)]
pub struct FrameReader {
    buf: Vec<u8>,
    max: usize,
    read_ahead: bool,
}

impl FrameReader {
    /// A reader for messages of at most `max` bytes that reads no byte
    /// past the bytes the framer needs. When `Frame::Need` never names
    /// more than the message, nothing after the message is consumed, so
    /// the stream can be handed on (to a TLS handshake, say).
    pub fn exact(max: usize) -> FrameReader {
        FrameReader {
            buf: Vec::new(),
            max,
            read_ahead: false,
        }
    }

    /// A reader for messages of at most `max` bytes that reads up to 4096
    /// bytes at a time; bytes after a message stay for the next call.
    pub fn read_ahead(max: usize) -> FrameReader {
        FrameReader {
            buf: Vec::new(),
            max,
            read_ahead: true,
        }
    }

    /// Read the next whole message.
    pub async fn next<R, F>(&mut self, conn: &mut R, framer: &mut F) -> Result<Vec<u8>, FrameError>
    where
        R: AsyncRead + Unpin + ?Sized,
        F: Framer + ?Sized,
    {
        loop {
            let have = self.buf.len();
            let need = match framer.frame(&self.buf) {
                Frame::Complete(len) | Frame::Need(len) if len > self.max => {
                    return Err(FrameError::TooLarge);
                }
                Frame::Complete(len) if len <= have => {
                    let rest = self.buf.split_off(len);
                    return Ok(std::mem::replace(&mut self.buf, rest));
                }
                Frame::Skip(n) if n > 0 => {
                    self.buf.drain(..n.min(have));
                    continue;
                }
                Frame::Invalid(reason) => return Err(FrameError::Invalid(reason)),
                // A framer that wants more than it has, or nothing at all:
                // read at least one more byte so the loop always moves on.
                Frame::Complete(len) | Frame::Need(len) => len.max(have + 1),
                Frame::Skip(_) => have + 1,
            };
            let want = if self.read_ahead {
                CHUNK
            } else {
                CHUNK.min(need - have)
            };
            let mut chunk = [0u8; CHUNK];
            let n = conn
                .read(&mut chunk[..want])
                .await
                .map_err(FrameError::Io)?;
            if n == 0 {
                return Err(FrameError::Closed { partial: have > 0 });
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::ReadBuf;

    /// A stream that hands out `reads` one per read call, then ends (or
    /// fails, with `fail`). Records how many bytes each read asked for.
    struct Script {
        reads: VecDeque<Vec<u8>>,
        fail: Option<io::ErrorKind>,
        asked: Vec<usize>,
    }

    impl Script {
        fn new(reads: Vec<&[u8]>) -> Script {
            Script {
                reads: reads.into_iter().map(<[u8]>::to_vec).collect(),
                fail: None,
                asked: Vec::new(),
            }
        }

        /// One byte per read.
        fn bytewise(bytes: &[u8]) -> Script {
            Script::new(bytes.chunks(1).collect())
        }
    }

    impl AsyncRead for Script {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            out: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            self.asked.push(out.remaining());
            match self.reads.pop_front() {
                Some(mut bytes) => {
                    let n = bytes.len().min(out.remaining());
                    out.put_slice(&bytes[..n]);
                    if n < bytes.len() {
                        self.reads.push_front(bytes.split_off(n));
                    }
                    Poll::Ready(Ok(()))
                }
                None => match self.fail {
                    Some(kind) => Poll::Ready(Err(io::Error::new(kind, "scripted"))),
                    None => Poll::Ready(Ok(())),
                },
            }
        }
    }

    /// A 2-byte big-endian length, then that many bytes.
    fn length_prefixed(buf: &[u8]) -> Frame {
        let [a, b, ..] = *buf else {
            return Frame::Need(2);
        };
        let total = 2 + usize::from(u16::from_be_bytes([a, b]));
        if buf.len() >= total {
            Frame::Complete(total)
        } else {
            Frame::Need(total)
        }
    }

    /// An 8-byte big-endian length, then that many bytes.
    fn u64_prefixed(buf: &[u8]) -> Frame {
        let Some(head) = buf.get(..8) else {
            return Frame::Need(8);
        };
        let len = u64::from_be_bytes(head.try_into().expect("get(..8) is 8 bytes"));
        let total = usize::try_from(len).map_or(usize::MAX, |l| l.saturating_add(8));
        if buf.len() >= total {
            Frame::Complete(total)
        } else {
            Frame::Need(total)
        }
    }

    #[tokio::test]
    async fn reads_whole_messages_and_keeps_the_rest() {
        let mut s = Script::new(vec![b"\x00\x03abc\x00\x01", b"z\x00"]);
        let mut r = FrameReader::read_ahead(100);
        let mut f = length_prefixed;
        assert_eq!(r.next(&mut s, &mut f).await.unwrap(), b"\x00\x03abc");
        assert_eq!(r.next(&mut s, &mut f).await.unwrap(), b"\x00\x01z");
        let e = r.next(&mut s, &mut f).await.unwrap_err();
        assert!(matches!(e, FrameError::Closed { partial: true }), "{e:?}");
    }

    #[tokio::test]
    async fn split_reads_one_byte_at_a_time() {
        let message = b"\x00\x05hello\x00\x02hi";
        for read_ahead in [false, true] {
            let mut s = Script::bytewise(message);
            let mut r = if read_ahead {
                FrameReader::read_ahead(100)
            } else {
                FrameReader::exact(100)
            };
            let mut f = length_prefixed;
            assert_eq!(r.next(&mut s, &mut f).await.unwrap(), b"\x00\x05hello");
            assert_eq!(r.next(&mut s, &mut f).await.unwrap(), b"\x00\x02hi");
        }
    }

    #[tokio::test]
    async fn framer_is_called_once_per_read() {
        let mut calls = 0;
        let mut f = |buf: &[u8]| {
            calls += 1;
            length_prefixed(buf)
        };
        let mut s = Script::bytewise(b"\x00\x03abc");
        let got = FrameReader::exact(100).next(&mut s, &mut f).await.unwrap();
        assert_eq!(got, b"\x00\x03abc");
        // Once before the first read, then once after each of the 5 reads.
        assert_eq!(calls, 6);
    }

    #[tokio::test]
    async fn exact_reads_nothing_past_the_message() {
        let mut s = Script::new(vec![b"\x00\x03abcAFTER"]);
        let mut r = FrameReader::exact(100);
        assert_eq!(
            r.next(&mut s, &mut length_prefixed).await.unwrap(),
            b"\x00\x03abc"
        );
        assert_eq!(s.asked, [2, 3]);
        assert_eq!(s.reads.pop_front().unwrap(), b"AFTER");
    }

    #[tokio::test]
    async fn length_above_the_cap_is_too_large_before_reading_it() {
        let mut s = Script::new(vec![b"\x00\x0b", b"0123456789a"]);
        let e = FrameReader::exact(12)
            .next(&mut s, &mut length_prefixed)
            .await
            .unwrap_err();
        assert!(matches!(e, FrameError::TooLarge), "{e:?}");
        assert_eq!(s.reads.len(), 1, "the body was not read");
        // Exactly at the cap is fine.
        let mut s = Script::new(vec![b"\x00\x0a", b"0123456789"]);
        let got = FrameReader::exact(12)
            .next(&mut s, &mut length_prefixed)
            .await;
        assert_eq!(got.unwrap().len(), 12);
    }

    #[tokio::test]
    async fn overflowing_length_is_too_large() {
        let mut s = Script::new(vec![&u64::MAX.to_be_bytes()]);
        let e = FrameReader::exact(65536)
            .next(&mut s, &mut u64_prefixed)
            .await
            .unwrap_err();
        assert!(matches!(e, FrameError::TooLarge), "{e:?}");
    }

    #[tokio::test]
    async fn complete_above_the_cap_is_too_large() {
        let mut s = Script::new(vec![b"0123456789"]);
        let mut f = |buf: &[u8]| match buf.len() {
            0 => Frame::Need(1),
            n => Frame::Complete(n),
        };
        let e = FrameReader::read_ahead(4)
            .next(&mut s, &mut f)
            .await
            .unwrap_err();
        assert!(matches!(e, FrameError::TooLarge), "{e:?}");
    }

    #[tokio::test]
    async fn eof_before_any_byte_and_inside_a_message() {
        let mut s = Script::new(vec![]);
        let e = FrameReader::exact(100)
            .next(&mut s, &mut length_prefixed)
            .await
            .unwrap_err();
        assert!(matches!(e, FrameError::Closed { partial: false }), "{e:?}");

        let mut s = Script::new(vec![b"\x00\x05he"]);
        let e = FrameReader::exact(100)
            .next(&mut s, &mut length_prefixed)
            .await
            .unwrap_err();
        assert!(matches!(e, FrameError::Closed { partial: true }), "{e:?}");
    }

    #[tokio::test]
    async fn read_errors_pass_through() {
        let mut s = Script::new(vec![b"\x00"]);
        s.fail = Some(io::ErrorKind::UnexpectedEof);
        let e = FrameReader::exact(100)
            .next(&mut s, &mut length_prefixed)
            .await
            .unwrap_err();
        assert!(
            matches!(&e, FrameError::Io(x) if x.kind() == io::ErrorKind::UnexpectedEof),
            "{e:?}"
        );

        let mut s = Script::new(vec![]);
        s.fail = Some(io::ErrorKind::ConnectionReset);
        let e = FrameReader::exact(100)
            .next(&mut s, &mut length_prefixed)
            .await
            .unwrap_err();
        assert!(
            matches!(&e, FrameError::Io(x) if x.kind() == io::ErrorKind::ConnectionReset),
            "{e:?}"
        );
    }

    #[tokio::test]
    async fn invalid_is_the_framers_reason() {
        let mut s = Script::new(vec![b"x"]);
        let mut f = |buf: &[u8]| match buf.first() {
            None => Frame::Need(1),
            Some(_) => Frame::Invalid("query: bad".into()),
        };
        let e = FrameReader::exact(100)
            .next(&mut s, &mut f)
            .await
            .unwrap_err();
        assert!(
            matches!(&e, FrameError::Invalid(r) if r == "query: bad"),
            "{e:?}"
        );
    }

    #[tokio::test]
    async fn skipped_bytes_are_dropped() {
        // Leading newlines are keepalives; a message is one line.
        let mut f = |buf: &[u8]| {
            let blank = buf.iter().take_while(|&&b| b == b'\n').count();
            if blank > 0 {
                return Frame::Skip(blank);
            }
            match buf.iter().position(|&b| b == b'\n') {
                Some(i) => Frame::Complete(i + 1),
                None => Frame::Need(buf.len() + 1),
            }
        };
        let mut s = Script::bytewise(b"\n\n\nab\n");
        let mut r = FrameReader::read_ahead(4);
        assert_eq!(r.next(&mut s, &mut f).await.unwrap(), b"ab\n");
        // Skipped bytes do not count as part of a message at EOF.
        let mut s = Script::new(vec![b"\n\n"]);
        let e = FrameReader::exact(4)
            .next(&mut s, &mut f)
            .await
            .unwrap_err();
        assert!(matches!(e, FrameError::Closed { partial: false }), "{e:?}");
    }

    #[tokio::test]
    async fn degenerate_answers_still_read_on() {
        // Need(0), Complete beyond the buffer and Skip(0) must not spin.
        let answers = [Frame::Need(0), Frame::Complete(3), Frame::Skip(0)];
        for answer in answers {
            let mut s = Script::new(vec![]);
            let mut f = |_: &[u8]| answer.clone();
            let e = FrameReader::exact(100)
                .next(&mut s, &mut f)
                .await
                .unwrap_err();
            assert!(matches!(e, FrameError::Closed { partial: false }), "{e:?}");
        }
    }
}
