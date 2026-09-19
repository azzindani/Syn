//! Minimal WebSocket client (RFC 6455), std only.
//!
//! This exists because the CDP hand needs a socket and `core` has no
//! dependencies. The protocol surface a DevTools client actually uses is
//! small: one handshake, text frames out, text frames in, answer pings.
//! Everything else (extensions, compression, server-side masking) is never
//! negotiated, so it never arrives.
//!
//! Generic over the stream, like `hand::Hand`, so every line below except
//! the connect path is exercised by the tests on any OS with no network.

use std::io::{BufReader, Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

/// The RFC 6455 handshake constant.
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Refuse a frame larger than this rather than allocating whatever a buggy
/// or hostile peer asks for. A DevTools reply that big is already unusable.
const MAX_FRAME: u64 = 16 * 1024 * 1024;

pub fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if c.len() > 1 { T[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if c.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// SHA-1, needed only to prove the peer answered our handshake key. It is
/// not used as a security primitive here and RFC 6455 does not treat it as
/// one either: the accept hash is a protocol check, not authentication.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for block in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, q) in block.chunks(4).take(16).enumerate() {
            w[i] = u32::from_be_bytes([q[0], q[1], q[2], q[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

/// The value a conforming server must return for `Sec-WebSocket-Key`.
pub fn accept_key(client_key: &str) -> String {
    b64(&sha1(format!("{client_key}{GUID}").as_bytes()))
}

/// xorshift64. Frame masking is an anti-cache-poisoning measure in RFC 6455,
/// not a confidentiality one, so a cheap PRNG is the right tool here and not
/// a shortcut. Nothing secret is derived from it.
#[derive(Debug)]
struct Rng(u64);

impl Rng {
    fn new() -> Self {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self(n | 1)
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 32) as u32
    }
}

/// An open WebSocket connection.
#[derive(Debug)]
pub struct Ws<S: Read + Write> {
    io: BufReader<S>,
    rng: Rng,
}

fn eof(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::UnexpectedEof, msg.to_string())
}

fn bad(msg: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg)
}

impl<S: Read + Write> Ws<S> {
    /// Perform the client handshake on an already-connected stream.
    ///
    /// The `Sec-WebSocket-Accept` echo is verified. Skipping that check is
    /// the usual shortcut, and it means a plain HTTP endpoint that happens
    /// to answer 101 gets treated as a socket; the failure then surfaces
    /// later as unreadable frames instead of here as a bad handshake.
    pub fn handshake(stream: S, host: &str, path: &str) -> std::io::Result<Self> {
        let mut rng = Rng::new();
        let mut nonce = [0u8; 16];
        for q in nonce.chunks_mut(4) {
            q.copy_from_slice(&rng.next_u32().to_be_bytes()[..q.len()]);
        }
        let key = b64(&nonce);
        let mut io = BufReader::new(stream);
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        io.get_mut().write_all(req.as_bytes())?;
        io.get_mut().flush()?;

        let mut status = String::new();
        read_line(&mut io, &mut status)?;
        if !status.contains(" 101") {
            return Err(bad(format!("websocket upgrade refused: {}", status.trim())));
        }
        let mut accept = None;
        loop {
            let mut line = String::new();
            read_line(&mut io, &mut line)?;
            let t = line.trim_end();
            if t.is_empty() {
                break;
            }
            if let Some((k, v)) = t.split_once(':')
                && k.trim().eq_ignore_ascii_case("sec-websocket-accept")
            {
                accept = Some(v.trim().to_string());
            }
        }
        let want = accept_key(&key);
        match accept {
            Some(got) if got == want => Ok(Self { io, rng }),
            Some(got) => Err(bad(format!("bad Sec-WebSocket-Accept: got {got}, want {want}"))),
            None => Err(bad("no Sec-WebSocket-Accept header: not a websocket server".into())),
        }
    }

    /// Wrap a stream that is ALREADY past the handshake. Tests drive
    /// framing with no server; real callers go through `handshake`.
    #[doc(hidden)]
    pub fn from_upgraded(stream: S) -> Self {
        Self { io: BufReader::new(stream), rng: Rng::new() }
    }

    fn frame(&mut self, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
        let mut out = Vec::with_capacity(payload.len() + 14);
        out.push(0x80 | opcode); // FIN
        let n = payload.len();
        // The mask bit is mandatory for a client.
        if n < 126 {
            out.push(0x80 | n as u8);
        } else if n <= u16::MAX as usize {
            out.push(0x80 | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        } else {
            out.push(0x80 | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
        let mask = self.rng.next_u32().to_be_bytes();
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        self.io.get_mut().write_all(&out)?;
        self.io.get_mut().flush()
    }

    pub fn send_text(&mut self, s: &str) -> std::io::Result<()> {
        self.frame(0x1, s.as_bytes())
    }

    pub fn close(&mut self) -> std::io::Result<()> {
        self.frame(0x8, &[])
    }

    /// Read the next text message, reassembling continuations and answering
    /// pings on the way. Control frames never surface to the caller.
    pub fn recv_text(&mut self) -> std::io::Result<String> {
        let mut buf: Vec<u8> = Vec::new();
        let mut assembling = false;
        loop {
            let (fin, opcode, payload) = self.read_frame()?;
            match opcode {
                0x9 => self.frame(0xA, &payload)?,
                0xA => {}
                0x8 => return Err(eof("peer closed the websocket")),
                0x1 | 0x0 => {
                    if opcode == 0x1 {
                        buf.clear();
                        assembling = true;
                    }
                    if !assembling {
                        return Err(bad("continuation frame with nothing to continue".into()));
                    }
                    buf.extend_from_slice(&payload);
                    if fin {
                        return Ok(String::from_utf8_lossy(&buf).into_owned());
                    }
                }
                // Binary is never negotiated with DevTools; drop it rather
                // than decoding bytes as text and reporting nonsense.
                _ => {}
            }
        }
    }

    fn read_frame(&mut self) -> std::io::Result<(bool, u8, Vec<u8>)> {
        let mut head = [0u8; 2];
        self.io.read_exact(&mut head)?;
        let fin = head[0] & 0x80 != 0;
        let opcode = head[0] & 0x0F;
        let masked = head[1] & 0x80 != 0;
        let len = match head[1] & 0x7F {
            126 => {
                let mut b = [0u8; 2];
                self.io.read_exact(&mut b)?;
                u16::from_be_bytes(b) as u64
            }
            127 => {
                let mut b = [0u8; 8];
                self.io.read_exact(&mut b)?;
                u64::from_be_bytes(b)
            }
            n => n as u64,
        };
        if len > MAX_FRAME {
            return Err(bad(format!("websocket frame of {len} bytes exceeds the {MAX_FRAME} cap")));
        }
        let mut mask = [0u8; 4];
        if masked {
            self.io.read_exact(&mut mask)?;
        }
        let mut payload = vec![0u8; len as usize];
        self.io.read_exact(&mut payload)?;
        if masked {
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= mask[i % 4];
            }
        }
        Ok((fin, opcode, payload))
    }
}

/// Read one CRLF line without pulling a `BufRead` bound into the struct.
fn read_line<S: Read>(io: &mut BufReader<S>, out: &mut String) -> std::io::Result<()> {
    let mut b = [0u8; 1];
    loop {
        if io.read(&mut b)? == 0 {
            return Err(eof("server hung up during the websocket handshake"));
        }
        out.push(b[0] as char);
        if b[0] == b'\n' {
            return Ok(());
        }
        if out.len() > 8192 {
            return Err(bad("handshake header line too long".into()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Duplex fake: scripted bytes in, everything written captured out.
    #[derive(Debug)]
    struct Fake {
        inbound: std::io::Cursor<Vec<u8>>,
        wrote: Rc<RefCell<Vec<u8>>>,
    }

    impl Read for Fake {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.inbound.read(buf)
        }
    }

    impl Write for Fake {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.wrote.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn ws(inbound: Vec<u8>) -> (Ws<Fake>, Rc<RefCell<Vec<u8>>>) {
        let wrote = Rc::new(RefCell::new(Vec::new()));
        let f = Fake { inbound: std::io::Cursor::new(inbound), wrote: Rc::clone(&wrote) };
        (Ws { io: BufReader::new(f), rng: Rng(12345) }, wrote)
    }

    /// An unmasked server text frame, as a real server sends it.
    fn server_text(s: &str) -> Vec<u8> {
        let b = s.as_bytes();
        let mut out = vec![0x81];
        if b.len() < 126 {
            out.push(b.len() as u8);
        } else {
            out.push(126);
            out.extend_from_slice(&(b.len() as u16).to_be_bytes());
        }
        out.extend_from_slice(b);
        out
    }

    #[test]
    fn sha1_matches_the_rfc_vectors() {
        assert_eq!(
            sha1(b"abc").iter().map(|b| format!("{b:02x}")).collect::<String>(),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            sha1(b"").iter().map(|b| format!("{b:02x}")).collect::<String>(),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
        // Crosses the 56-byte padding boundary, where a hand-rolled SHA-1
        // usually breaks.
        assert_eq!(
            sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn b64_matches_the_rfc_4648_vectors() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foo"), "Zm9v");
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn accept_key_matches_the_rfc_6455_example() {
        // RFC 6455 section 1.3, the worked example.
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    /// A fake that answers the handshake the way a real server does: it
    /// reads the key the client actually generated and hashes it. Canned
    /// bytes cannot do this, and without it the accept check is untested.
    #[derive(Debug)]
    struct Server {
        req: Vec<u8>,
        resp: std::io::Cursor<Vec<u8>>,
        after: Vec<u8>,
    }

    impl Read for Server {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.resp.get_ref().is_empty() {
                let req = String::from_utf8_lossy(&self.req).to_string();
                let key = req
                    .lines()
                    .find_map(|l| l.strip_prefix("Sec-WebSocket-Key: "))
                    .expect("client must send a key")
                    .trim()
                    .to_string();
                let mut r = format!(
                    "HTTP/1.1 101 Switching Protocols
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Accept: {}

",
                    accept_key(&key)
                )
                .into_bytes();
                r.extend_from_slice(&self.after);
                self.resp = std::io::Cursor::new(r);
            }
            self.resp.read(buf)
        }
    }

    impl Write for Server {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.req.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn handshake_accepts_a_conforming_server_and_then_talks() {
        let s = Server {
            req: Vec::new(),
            resp: std::io::Cursor::new(Vec::new()),
            after: server_text("{\"id\":7,\"result\":{}}"),
        };
        let mut w = Ws::handshake(s, "127.0.0.1:9222", "/devtools/browser/abc").unwrap();
        // The connection is usable straight after the upgrade: the reader
        // must not have eaten the first frame while consuming headers.
        assert_eq!(w.recv_text().unwrap(), "{\"id\":7,\"result\":{}}");
    }

    #[test]
    fn handshake_rejects_a_wrong_accept_hash() {
        let resp = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: wrong\r\n\r\n";
        let wrote = Rc::new(RefCell::new(Vec::new()));
        let f = Fake { inbound: std::io::Cursor::new(resp.as_bytes().to_vec()), wrote: Rc::clone(&wrote) };
        let e = Ws::handshake(f, "127.0.0.1:9222", "/devtools/browser/x").unwrap_err();
        assert!(e.to_string().contains("bad Sec-WebSocket-Accept"), "{e}");
        // The request itself must still be a well-formed upgrade.
        let sent = String::from_utf8(wrote.borrow().clone()).unwrap();
        assert!(sent.starts_with("GET /devtools/browser/x HTTP/1.1\r\n"));
        assert!(sent.contains("Upgrade: websocket"));
        assert!(sent.contains("Sec-WebSocket-Version: 13"));
    }

    #[test]
    fn handshake_rejects_a_plain_http_endpoint() {
        let resp = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
        let wrote = Rc::new(RefCell::new(Vec::new()));
        let f = Fake { inbound: std::io::Cursor::new(resp.as_bytes().to_vec()), wrote };
        let e = Ws::handshake(f, "h", "/").unwrap_err();
        assert!(e.to_string().contains("upgrade refused"), "{e}");
    }

    #[test]
    fn handshake_without_the_accept_header_is_refused() {
        let resp = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\r\n";
        let wrote = Rc::new(RefCell::new(Vec::new()));
        let f = Fake { inbound: std::io::Cursor::new(resp.as_bytes().to_vec()), wrote };
        let e = Ws::handshake(f, "h", "/").unwrap_err();
        assert!(e.to_string().contains("not a websocket server"), "{e}");
    }

    #[test]
    fn a_client_frame_is_masked_and_round_trips() {
        let (mut w, wrote) = ws(Vec::new());
        w.send_text("hello").unwrap();
        let out = wrote.borrow().clone();
        assert_eq!(out[0], 0x81, "FIN + text opcode");
        assert_eq!(out[1] & 0x80, 0x80, "a client frame MUST set the mask bit");
        assert_eq!(out[1] & 0x7F, 5);
        let mask = &out[2..6];
        let body: Vec<u8> = out[6..].iter().enumerate().map(|(i, b)| b ^ mask[i % 4]).collect();
        assert_eq!(String::from_utf8(body).unwrap(), "hello");
    }

    #[test]
    fn a_long_frame_uses_the_extended_length() {
        let (mut w, wrote) = ws(Vec::new());
        let big = "x".repeat(1000);
        w.send_text(&big).unwrap();
        let out = wrote.borrow().clone();
        assert_eq!(out[1] & 0x7F, 126, "1000 bytes needs the 16-bit length");
        assert_eq!(u16::from_be_bytes([out[2], out[3]]), 1000);
    }

    #[test]
    fn recv_reads_a_server_text_frame() {
        let (mut w, _) = ws(server_text("{\"id\":1}"));
        assert_eq!(w.recv_text().unwrap(), "{\"id\":1}");
    }

    #[test]
    fn recv_reassembles_continuations() {
        let mut inbound = vec![0x01, 0x03];
        inbound.extend_from_slice(b"abc"); // text, not final
        inbound.extend_from_slice(&[0x00, 0x03]);
        inbound.extend_from_slice(b"def"); // continuation, not final
        inbound.extend_from_slice(&[0x80, 0x03]);
        inbound.extend_from_slice(b"ghi"); // continuation, final
        let (mut w, _) = ws(inbound);
        assert_eq!(w.recv_text().unwrap(), "abcdefghi");
    }

    #[test]
    fn a_ping_is_answered_and_never_surfaces() {
        let mut inbound = vec![0x89, 0x02, 0xAA, 0xBB]; // ping
        inbound.extend_from_slice(&server_text("after"));
        let (mut w, wrote) = ws(inbound);
        assert_eq!(w.recv_text().unwrap(), "after");
        let out = wrote.borrow().clone();
        assert_eq!(out[0], 0x8A, "must answer with a pong");
        let mask = &out[2..6];
        let body: Vec<u8> = out[6..].iter().enumerate().map(|(i, b)| b ^ mask[i % 4]).collect();
        assert_eq!(body, vec![0xAA, 0xBB], "a pong echoes the ping payload");
    }

    #[test]
    fn a_close_frame_is_an_error_not_an_empty_message() {
        let (mut w, _) = ws(vec![0x88, 0x00]);
        let e = w.recv_text().unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof);
        assert!(e.to_string().contains("closed the websocket"));
    }

    #[test]
    fn an_oversized_frame_is_refused_before_allocating() {
        let mut inbound = vec![0x81, 127];
        inbound.extend_from_slice(&(u64::MAX / 2).to_be_bytes());
        let (mut w, _) = ws(inbound);
        let e = w.recv_text().unwrap_err();
        assert!(e.to_string().contains("exceeds the"), "{e}");
    }

    #[test]
    fn a_masked_server_frame_is_still_unmasked_correctly() {
        // Servers must not mask, but decoding it costs four lines and the
        // alternative is silently returning ciphertext as page content.
        let mask = [1u8, 2, 3, 4];
        let body = b"hi";
        let mut inbound = vec![0x81, 0x80 | 2];
        inbound.extend_from_slice(&mask);
        inbound.extend(body.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        let (mut w, _) = ws(inbound);
        assert_eq!(w.recv_text().unwrap(), "hi");
    }
}
