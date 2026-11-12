//! The parts of the redis protocol (RESP2) the redis plugin needs.

use tokio::io::{AsyncRead, AsyncReadExt};

/// Largest bulk string accepted; replies to AUTH, PING and ROLE are small.
const MAX_BULK: usize = 1 << 20;
/// Largest array accepted (ROLE on a master lists its replicas).
const MAX_ARRAY: usize = 4096;
/// Deepest nesting accepted.
const MAX_DEPTH: usize = 8;

/// One reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Simple(String),
    Error(String),
    Integer(i64),
    Bulk(Option<Vec<u8>>),
    Array(Option<Vec<Reply>>),
}

/// A reply that does not follow RESP2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError(pub &'static str);

/// Encode a command as an array of bulk strings.
pub fn encode(args: &[&[u8]]) -> Vec<u8> {
    let mut out = format!("*{}\r\n", args.len()).into_bytes();
    for arg in args {
        out.extend_from_slice(format!("${}\r\n", arg.len()).as_bytes());
        out.extend_from_slice(arg);
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// Parse one reply from the start of `buf`: `Ok(None)` when more bytes are
/// needed, otherwise the reply and the number of bytes it took.
pub fn parse(buf: &[u8]) -> Result<Option<(Reply, usize)>, ProtocolError> {
    parse_at(buf, 0)
}

fn parse_at(buf: &[u8], depth: usize) -> Result<Option<(Reply, usize)>, ProtocolError> {
    if depth > MAX_DEPTH {
        return Err(ProtocolError("nested too deeply"));
    }
    let Some(end) = buf.windows(2).position(|w| w == b"\r\n") else {
        if buf.len() > MAX_BULK {
            return Err(ProtocolError("line too long"));
        }
        return Ok(None);
    };
    let Some((&kind, rest)) = buf[..end].split_first() else {
        return Err(ProtocolError("empty line"));
    };
    let line = std::str::from_utf8(rest).map_err(|_| ProtocolError("line is not UTF-8"))?;
    let after = end + 2;
    match kind {
        b'+' => Ok(Some((Reply::Simple(line.to_owned()), after))),
        b'-' => Ok(Some((Reply::Error(line.to_owned()), after))),
        b':' => {
            let n = line.parse().map_err(|_| ProtocolError("bad integer"))?;
            Ok(Some((Reply::Integer(n), after)))
        }
        b'$' => {
            let len: i64 = line.parse().map_err(|_| ProtocolError("bad bulk length"))?;
            if len == -1 {
                return Ok(Some((Reply::Bulk(None), after)));
            }
            let len = usize::try_from(len).map_err(|_| ProtocolError("bad bulk length"))?;
            if len > MAX_BULK {
                return Err(ProtocolError("bulk string too long"));
            }
            if buf.len() < after + len + 2 {
                return Ok(None);
            }
            if &buf[after + len..after + len + 2] != b"\r\n" {
                return Err(ProtocolError("bulk string not terminated"));
            }
            Ok(Some((
                Reply::Bulk(Some(buf[after..after + len].to_vec())),
                after + len + 2,
            )))
        }
        b'*' => {
            let len: i64 = line
                .parse()
                .map_err(|_| ProtocolError("bad array length"))?;
            if len == -1 {
                return Ok(Some((Reply::Array(None), after)));
            }
            let len = usize::try_from(len).map_err(|_| ProtocolError("bad array length"))?;
            if len > MAX_ARRAY {
                return Err(ProtocolError("array too long"));
            }
            let mut items = Vec::with_capacity(len);
            let mut pos = after;
            for _ in 0..len {
                match parse_at(&buf[pos..], depth + 1)? {
                    Some((item, used)) => {
                        items.push(item);
                        pos += used;
                    }
                    None => return Ok(None),
                }
            }
            Ok(Some((Reply::Array(Some(items)), pos)))
        }
        _ => Err(ProtocolError("unknown reply type")),
    }
}

/// Why reading a reply failed.
#[derive(Debug)]
pub enum ReadError {
    Io(std::io::Error),
    Closed,
    Protocol(ProtocolError),
}

/// Reads replies from a connection, keeping bytes that arrive early.
pub struct Reader {
    buf: Vec<u8>,
}

impl Reader {
    pub fn new() -> Reader {
        Reader { buf: Vec::new() }
    }

    /// Read the next reply.
    pub async fn read<R: AsyncRead + Unpin + ?Sized>(
        &mut self,
        conn: &mut R,
    ) -> Result<Reply, ReadError> {
        loop {
            if let Some((reply, used)) = parse(&self.buf).map_err(ReadError::Protocol)? {
                self.buf.drain(..used);
                return Ok(reply);
            }
            let mut chunk = [0u8; 4096];
            let n = conn.read(&mut chunk).await.map_err(ReadError::Io)?;
            if n == 0 {
                return Err(ReadError::Closed);
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }
}

impl Default for Reader {
    fn default() -> Self {
        Reader::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bulk(s: &str) -> Reply {
        Reply::Bulk(Some(s.as_bytes().to_vec()))
    }

    #[test]
    fn encodes_commands() {
        assert_eq!(encode(&[b"PING"]), b"*1\r\n$4\r\nPING\r\n");
        assert_eq!(
            encode(&[b"AUTH", b"emfor", b"p w"]),
            b"*3\r\n$4\r\nAUTH\r\n$5\r\nemfor\r\n$3\r\np w\r\n"
        );
    }

    #[test]
    fn parses_each_reply_type() {
        let cases: Vec<(&[u8], Reply)> = vec![
            (b"+PONG\r\n", Reply::Simple("PONG".into())),
            (b"-WRONGPASS invalid\r\n", Reply::Error("WRONGPASS invalid".into())),
            (b":42\r\n", Reply::Integer(42)),
            (b"$5\r\nslave\r\n", bulk("slave")),
            (b"$-1\r\n", Reply::Bulk(None)),
            (b"*-1\r\n", Reply::Array(None)),
            (
                b"*3\r\n$6\r\nmaster\r\n:3129\r\n*1\r\n*3\r\n$9\r\n127.0.0.1\r\n$4\r\n9001\r\n$4\r\n3129\r\n",
                Reply::Array(Some(vec![
                    bulk("master"),
                    Reply::Integer(3129),
                    Reply::Array(Some(vec![Reply::Array(Some(vec![
                        bulk("127.0.0.1"),
                        bulk("9001"),
                        bulk("3129"),
                    ]))])),
                ])),
            ),
        ];
        for (input, want) in cases {
            let (reply, used) = parse(input).unwrap().unwrap();
            assert_eq!(reply, want, "{:?}", String::from_utf8_lossy(input));
            assert_eq!(used, input.len());
        }
    }

    #[test]
    fn incomplete_input_needs_more() {
        let full = b"*2\r\n$5\r\nslave\r\n$9\r\n127.0.0.1\r\n";
        for cut in 0..full.len() {
            assert_eq!(parse(&full[..cut]), Ok(None), "cut at {cut}");
        }
        assert!(parse(full).unwrap().is_some());
    }

    #[test]
    fn rejects_bad_input() {
        for input in [
            &b"?x\r\n"[..],
            b"$abc\r\n",
            b"$3\r\nabcde\r\n",
            b":1.5\r\n",
            b"*99999999\r\n",
            b"$99999999\r\n",
            b"\r\n",
        ] {
            assert!(
                parse(input).is_err(),
                "{:?}",
                String::from_utf8_lossy(input)
            );
        }
    }

    #[tokio::test]
    async fn reader_keeps_extra_bytes() {
        let mut input: &[u8] = b"+OK\r\n+PONG\r\n";
        let mut r = Reader::new();
        assert_eq!(
            r.read(&mut input).await.unwrap(),
            Reply::Simple("OK".into())
        );
        assert_eq!(
            r.read(&mut input).await.unwrap(),
            Reply::Simple("PONG".into())
        );
        assert!(matches!(r.read(&mut input).await, Err(ReadError::Closed)));
    }
}
