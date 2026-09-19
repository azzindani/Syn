//! The hand: `office-rpc/1` transport from the Rust core to a live sidecar.
//!
//! This is the seam that was missing. `mcpgate::rpc_request` could already
//! build an envelope and nothing could send one, so the brain and the hands
//! were two halves wired to the same protocol with nothing in between. A
//! `Hand` is that in-between: one line of JSON out, one line of JSON back,
//! over whatever duplex stream it is handed.
//!
//! The stream is generic so the protocol can be tested without Office and
//! without Windows — the tests below drive a fake duplex, and CI on Linux
//! and macOS exercises every line except `connect`. On Windows a named pipe
//! is just a file, so `connect` is an ordinary open of `\\.\pipe\<name>`.
//!
//! Framing matches the sidecar exactly: byte mode, UTF-8 with no BOM, one
//! object per line. Both of those were learned the hard way against live
//! Excel; see `docs/runbook-windows.md`.

use crate::ops::{Call, ExportArgs, FormatArgs, ReadArgs, StructArgs, WriteArgs};
use std::io::{BufRead, BufReader, Read, Write};

/// One decoded sidecar reply. `ok` false carries `error` instead of `preview`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub ok: bool,
    pub preview: String,
    pub error: String,
}

impl Reply {
    /// Collapse to the caller's result shape: Ok(preview) or Err(error).
    pub fn into_result(self) -> Result<String, String> {
        if self.ok { Ok(self.preview) } else { Err(self.error) }
    }
}

/// Escape a string for embedding in the hand-rolled JSON below.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Read one JSON string field. Mirrors the sidecar's own flat parser: no
/// nesting, no arrays, which is all `office-rpc/1` replies ever contain.
fn field(json: &str, name: &str) -> Option<String> {
    let key = format!("\"{name}\"");
    let i = json.find(&key)?;
    let rest = json[i + key.len()..].trim_start_matches([' ', ':']);
    let rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut it = rest.chars();
    while let Some(c) = it.next() {
        match c {
            '\\' => match it.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => break,
            },
            '"' => return Some(out),
            c => out.push(c),
        }
    }
    None
}

/// Decode one reply line. Anything unparseable is reported as an error
/// rather than silently treated as success.
pub fn parse_reply(line: &str) -> Reply {
    let ok = line.contains("\"ok\":true") || line.contains("\"ok\": true");
    if ok {
        Reply { ok: true, preview: field(line, "preview").unwrap_or_default(), error: String::new() }
    } else {
        let error = field(line, "error")
            .unwrap_or_else(|| format!("unparseable sidecar reply: {}", crate::security::truncate_output(line)));
        Reply { ok: false, preview: String::new(), error }
    }
}

/// Build an `office-rpc/1` envelope. `payload` is a top-level field because
/// that is where the sidecar's write path reads it from.
pub fn envelope(method: &str, handle: &str, args_json: &str, payload: Option<&str>) -> String {
    let mut s = format!(
        "{{\"jsonrpc\":\"office-rpc/1\",\"method\":\"{}\",\"handle\":\"{}\",\"args\":{}",
        esc(method),
        esc(handle),
        args_json
    );
    if let Some(p) = payload {
        s.push_str(&format!(",\"payload\":\"{}\"", esc(p)));
    }
    s.push('}');
    s
}

/// Serialise a grid the way the sidecar's write path parses it: cells joined
/// by `|` and rows by `;`, with a separator inside a value escaped so it
/// arrives as part of the value rather than splitting it.
pub fn grid_payload(values: &[Vec<String>]) -> String {
    fn cell(c: &str) -> String {
        // Backslash first, or escaping the separators would escape the
        // escapes this adds.
        c.replace('\\', "\\\\").replace('|', "\\|").replace(';', "\\;")
    }
    values
        .iter()
        .map(|r| r.iter().map(|c| cell(c)).collect::<Vec<_>>().join("|"))
        .collect::<Vec<_>>()
        .join(";")
}

/// Map one primitive op onto a sidecar method + envelope.
///
/// `Read`, `Write`, `Export`, `Undo`, `Format` and the `AddSheet`, `Pivot`,
/// `Chart`, `Invoke` and `InsertParagraph` verbs map. The rest return None:
/// the sidecar has no method for them, and inventing a silent no-op would
/// let a caller believe a change landed in a live document when nothing
/// happened.
pub fn envelope_for(call: &Call, handle: &str) -> Option<String> {
    match call {
        Call::Read(ReadArgs { selector }) => {
            Some(envelope("read", handle, &format!("{{\"selector\":\"{}\"}}", esc(selector)), None))
        }
        Call::Write(WriteArgs { selector, values }) => Some(envelope(
            "write",
            handle,
            &format!("{{\"selector\":\"{}\"}}", esc(selector)),
            Some(&grid_payload(values)),
        )),
        Call::Export(ExportArgs { format, path, .. }) => Some(envelope(
            "export",
            handle,
            &format!(
                "{{\"format\":\"{}\",\"path\":\"{}\"}}",
                esc(format),
                esc(path.as_deref().unwrap_or(""))
            ),
            None,
        )),
        Call::Struct(StructArgs::Invoke { selector, action }) => Some(envelope(
            "invoke",
            handle,
            &format!("{{\"selector\":\"{}\",\"action\":\"{}\"}}", esc(selector), esc(action)),
            None,
        )),
        Call::Undo => Some(envelope("undo", handle, "{}", None)),
        // Style as a payload in the same key=value shape the tool takes, so
        // the sidecar reads what the model wrote without a second grammar.
        Call::Format(FormatArgs { selector, style }) => Some(envelope(
            "format",
            handle,
            &format!("{{\"selector\":\"{}\"}}", esc(selector)),
            Some(
                &style
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(";"),
            ),
        )),
        Call::Struct(StructArgs::AddSheet { name }) => {
            Some(envelope("addSheet", handle, &format!("{{\"name\":\"{}\"}}", esc(name)), None))
        }
        Call::Struct(StructArgs::Pivot { source, rows, cols, values, at }) => Some(envelope(
            "pivot",
            handle,
            &format!(
                "{{\"source\":\"{}\",\"rows\":\"{}\",\"cols\":\"{}\",\"values\":\"{}\",\"at\":\"{}\"}}",
                esc(source),
                esc(rows),
                esc(cols),
                esc(values),
                esc(at)
            ),
            None,
        )),
        Call::Struct(StructArgs::Chart { kind, source, title, at, style }) => Some(envelope(
            "chart",
            handle,
            &format!(
                "{{\"kind\":\"{}\",\"source\":\"{}\",\"title\":\"{}\",\"at\":\"{}\"}}",
                esc(kind),
                esc(source),
                esc(title),
                esc(at)
            ),
            (!style.is_empty()).then_some(style.as_str()),
        )),
        Call::Struct(StructArgs::Table { source, name }) => Some(envelope(
            "table",
            handle,
            &format!("{{\"source\":\"{}\",\"name\":\"{}\"}}", esc(source), esc(name)),
            None,
        )),
        Call::Struct(StructArgs::Name { name, at }) => Some(envelope(
            "name",
            handle,
            &format!("{{\"name\":\"{}\",\"at\":\"{}\"}}", esc(name), esc(at)),
            None,
        )),
        // The rule rides as the payload, the way a style does for `format`.
        Call::Struct(StructArgs::Conditional { selector, rule }) => Some(envelope(
            "conditional",
            handle,
            &format!("{{\"selector\":\"{}\"}}", esc(selector)),
            Some(rule),
        )),
        // `rows` carries the field: the sidecar reads pivot args from the
        // same envelope shape and this keeps one grammar rather than two.
        Call::Struct(StructArgs::Slicer { pivot, field, at }) => Some(envelope(
            "slicer",
            handle,
            &format!(
                "{{\"name\":\"{}\",\"rows\":\"{}\",\"at\":\"{}\"}}",
                esc(pivot),
                esc(field),
                esc(at)
            ),
            None,
        )),
        Call::Struct(StructArgs::InsertParagraph { text, style }) => Some(envelope(
            "insertParagraph",
            handle,
            &format!("{{\"name\":\"{}\"}}", esc(style)),
            Some(text),
        )),
        Call::Struct(StructArgs::InsertTable { rows, style }) => Some(envelope(
            "insertTable",
            handle,
            &format!("{{\"name\":\"{}\"}}", esc(style)),
            Some(&grid_payload(rows)),
        )),
        Call::Struct(StructArgs::PageBreak { kind }) => Some(envelope(
            "pageBreak",
            handle,
            &format!("{{\"name\":\"{}\"}}", esc(kind)),
            None,
        )),
        Call::Struct(StructArgs::Contents { title }) => Some(envelope(
            "contents",
            handle,
            &format!("{{\"title\":\"{}\"}}", esc(title)),
            None,
        )),
        Call::Struct(StructArgs::PageNumbers { text }) => Some(envelope(
            "pageNumbers",
            handle,
            &format!("{{\"text\":\"{}\"}}", esc(text)),
            None,
        )),
        Call::Struct(StructArgs::Picture { path, width }) => Some(envelope(
            "picture",
            handle,
            &format!("{{\"text\":\"{}\",\"name\":\"{}\"}}", esc(path), esc(width)),
            None,
        )),
        Call::Struct(_) => None,
    }
}

/// A connected hand: one sidecar, one app, many handles.
#[derive(Debug)]
pub struct Hand<S: Read + Write> {
    io: BufReader<S>,
    calls: u64,
}

impl<S: Read + Write> Hand<S> {
    pub fn new(stream: S) -> Self {
        Self { io: BufReader::new(stream), calls: 0 }
    }

    /// Calls issued on this hand (the widget shows it per sidecar).
    pub fn calls(&self) -> u64 {
        self.calls
    }

    /// Send one envelope, read one reply. Blocking: the sidecar's message
    /// filter bounds the far side, so a wedged app returns an error reply
    /// rather than leaving this read outstanding forever.
    pub fn send(&mut self, envelope: &str) -> std::io::Result<Reply> {
        let w = self.io.get_mut();
        w.write_all(envelope.as_bytes())?;
        w.write_all(b"\n")?;
        w.flush()?;
        self.calls += 1;
        let mut line = String::new();
        if self.io.read_line(&mut line)? == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "sidecar closed the pipe (crashed, or the app was quit)",
            ));
        }
        Ok(parse_reply(line.trim_end()))
    }

    /// Dispatch one primitive op to the live document.
    pub fn dispatch(&mut self, call: &Call, handle: &str) -> std::io::Result<Reply> {
        match envelope_for(call, handle) {
            Some(e) => self.send(&e),
            None => Ok(Reply {
                ok: false,
                preview: String::new(),
                error: "op not implemented by the live hand yet (format/struct)".into(),
            }),
        }
    }
}

/// The runner's view of a hand: dispatch one op, get one reply.
///
/// A trait object so `Runner` never names the stream type — it owns a hand
/// without caring whether it is a real pipe or a test double, which is what
/// lets the live dispatch path be tested off Windows.
pub trait LiveHand: std::fmt::Debug {
    fn dispatch_call(&mut self, call: &Call, handle: &str) -> std::io::Result<Reply>;
}

impl<S: Read + Write + std::fmt::Debug> LiveHand for Hand<S> {
    fn dispatch_call(&mut self, call: &Call, handle: &str) -> std::io::Result<Reply> {
        self.dispatch(call, handle)
    }
}

/// Windows named pipe path for a sidecar name.
pub fn pipe_path(name: &str) -> String {
    format!(r"\\.\pipe\{name}")
}

impl Hand<std::fs::File> {
    /// Connect to a running sidecar by pipe name.
    ///
    /// A Windows named pipe opens like any other file once the server is
    /// listening. The server accepts a single client, so a second connect
    /// fails until the first disconnects — that is the sidecar's limit, not
    /// this client's, and the error says so.
    pub fn connect(name: &str) -> std::io::Result<Self> {
        let path = pipe_path(name);
        let file = std::fs::OpenOptions::new().read(true).write(true).open(&path).map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("cannot open {path}: {e}. Is office-host running with --pipe {name}, and free?"),
            )
        })?;
        Ok(Self::new(file))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Duplex fake: canned replies in, everything written captured out.
    struct Fake {
        replies: std::io::Cursor<Vec<u8>>,
        wrote: Rc<RefCell<Vec<u8>>>,
    }

    impl Read for Fake {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.replies.read(buf)
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

    fn hand(replies: &str) -> (Hand<Fake>, Rc<RefCell<Vec<u8>>>) {
        let wrote = Rc::new(RefCell::new(Vec::new()));
        let f = Fake { replies: std::io::Cursor::new(replies.as_bytes().to_vec()), wrote: Rc::clone(&wrote) };
        (Hand::new(f), wrote)
    }

    fn sent(w: &Rc<RefCell<Vec<u8>>>) -> String {
        String::from_utf8(w.borrow().clone()).unwrap()
    }

    #[test]
    fn read_round_trip_matches_the_live_wire_format() {
        // Exactly the bytes real Excel answered with in the smoke run.
        let (mut h, w) = hand("{\"ok\":true,\"preview\":\"grid Sheet1: 2x2\"}\n");
        let call = Call::Read(ReadArgs { selector: "Sheet1!A1:B2".into() });
        let r = h.dispatch(&call, "excel:plan.xlsx:Sheet1").unwrap();
        assert_eq!(r.preview, "grid Sheet1: 2x2");
        assert!(r.ok);
        let out = sent(&w);
        assert!(out.ends_with('\n'), "must be line framed");
        assert!(out.contains("\"jsonrpc\":\"office-rpc/1\""));
        assert!(out.contains("\"method\":\"read\""));
        assert!(out.contains("\"handle\":\"excel:plan.xlsx:Sheet1\""));
        assert!(out.contains("\"selector\":\"Sheet1!A1:B2\""));
        assert_eq!(h.calls(), 1);
    }

    #[test]
    fn write_sends_grid_payload() {
        let (mut h, w) = hand("{\"ok\":true,\"preview\":\"wrote 2x2 at Sheet1!A1\"}\n");
        let call = Call::Write(WriteArgs {
            selector: "Sheet1!A1".into(),
            values: vec![vec!["a".into(), "b".into()], vec!["c".into(), "d".into()]],
        });
        assert!(h.dispatch(&call, "excel:p.xlsx:Sheet1").unwrap().ok);
        assert!(sent(&w).contains("\"payload\":\"a|b;c|d\""));
    }

    #[test]
    fn error_reply_is_surfaced_not_swallowed() {
        let (mut h, _) = hand("{\"ok\":false,\"error\":\"workbook not open for excel:nosuch.xlsx:Sheet1\"}\n");
        let r = h.dispatch(&Call::Read(ReadArgs { selector: "Sheet1!A1".into() }), "excel:nosuch.xlsx:Sheet1").unwrap();
        assert!(!r.ok);
        assert!(r.error.contains("not open"));
        assert!(r.into_result().is_err());
    }

    #[test]
    fn unparseable_reply_fails_closed() {
        let (mut h, _) = hand("not json at all\n");
        let r = h.dispatch(&Call::Undo, "excel:p.xlsx:Sheet1").unwrap();
        assert!(!r.ok, "a reply we cannot read must never count as success");
        assert!(r.error.contains("unparseable"));
    }

    #[test]
    fn closed_pipe_is_an_error_not_a_silent_success() {
        let (mut h, _) = hand(""); // server hung up
        let e = h.dispatch(&Call::Undo, "h").unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof);
        assert!(e.to_string().contains("closed the pipe"));
    }

    #[test]
    fn unsupported_ops_refuse_rather_than_pretend() {
        use crate::ops::StructArgs;
        // What is still unmapped must refuse, and must put nothing on the
        // wire: a silent no-op would report a change that never happened.
        let (mut h, w) = hand("");
        let s = Call::Struct(StructArgs::CreateSlide { title: "T".into(), bullets: vec![] });
        assert!(!h.dispatch(&s, "ppt:d.pptx:deck").unwrap().ok);
        assert!(sent(&w).is_empty(), "nothing may go on the wire for an unmapped op");
        let t = Call::Struct(StructArgs::TrackChange { para: None, text: "t".into() });
        assert!(!h.dispatch(&t, "word:d.docx:body").unwrap().ok);
        assert!(sent(&w).is_empty());
    }

    #[test]
    fn a_word_report_can_be_built_rather_than_filled_into_a_template() {
        use crate::ops::StructArgs;
        // Word could only overwrite paragraphs that already existed, so a
        // report had to ship as a template with the right number of blank
        // lines in it. These are the verbs that write the document instead.
        let (mut h, w) = hand(&"{\"ok\":true,\"detail\":\"done\"}\n".repeat(6));
        let calls = [
            Call::Struct(StructArgs::InsertParagraph {
                text: "Estate performance".into(),
                style: "Heading 1".into(),
            }),
            Call::Struct(StructArgs::InsertTable {
                rows: vec![vec!["Site".into(), "kWh".into()], vec!["Bearspaw".into(), "3082638".into()]],
                style: String::new(),
            }),
            Call::Struct(StructArgs::PageBreak { kind: "page".into() }),
            Call::Struct(StructArgs::Contents { title: "Contents".into() }),
            Call::Struct(StructArgs::PageNumbers { text: "Quarterly review".into() }),
            Call::Struct(StructArgs::Picture { path: "out\\by-site.png".into(), width: "420".into() }),
        ];
        for c in &calls {
            assert!(h.dispatch(c, "word:r.docx:body").unwrap().ok, "{c:?} did not reach the hand");
        }
        let out = sent(&w);
        for method in ["insertParagraph", "insertTable", "pageBreak", "contents", "pageNumbers", "picture"] {
            assert!(out.contains(&format!("\"method\":\"{method}\"")), "{method} never went on the wire: {out}");
        }
        // The style rides in `name`, which is the same field addSheet and
        // table already use, so the sidecar needs no second grammar.
        assert!(out.contains("\"name\":\"Heading 1\""), "{out}");
        // A table crosses as the same pipe/semicolon grid the sheet uses.
        assert!(out.contains("Site|kWh;Bearspaw|3082638"), "{out}");
    }

    #[test]
    fn several_calls_reuse_one_connection() {
        let (mut h, w) = hand("{\"ok\":true,\"preview\":\"one\"}\n{\"ok\":true,\"preview\":\"two\"}\n");
        let c = Call::Read(ReadArgs { selector: "Sheet1!A1".into() });
        assert_eq!(h.dispatch(&c, "x").unwrap().preview, "one");
        assert_eq!(h.dispatch(&c, "x").unwrap().preview, "two");
        assert_eq!(h.calls(), 2);
        assert_eq!(sent(&w).lines().count(), 2);
    }

    #[test]
    fn quotes_and_newlines_survive_escaping() {
        let (mut h, w) = hand("{\"ok\":true,\"preview\":\"said \\\"hi\\\"\\nand left\"}\n");
        let call = Call::Write(WriteArgs {
            selector: "Sheet1!A1".into(),
            values: vec![vec!["say \"hi\"".into()]],
        });
        let r = h.dispatch(&call, "excel:p.xlsx:Sheet1").unwrap();
        assert_eq!(r.preview, "said \"hi\"\nand left");
        let out = sent(&w);
        assert!(out.contains("\\\"hi\\\""), "quotes must be escaped on the wire");
        assert_eq!(out.lines().count(), 1, "an escaped newline must not split the frame");
    }

    #[test]
    fn export_carries_format_and_path() {
        let (mut h, w) = hand("{\"ok\":true,\"preview\":\"exported out.xlsx\"}\n");
        let call = Call::Export(ExportArgs { format: "xlsx".into(), path: Some("out.xlsx".into()), sheet: None });
        assert!(h.dispatch(&call, "excel:p.xlsx:Sheet1").unwrap().ok);
        let out = sent(&w);
        assert!(out.contains("\"format\":\"xlsx\""));
        assert!(out.contains("\"path\":\"out.xlsx\""));
    }

    #[test]
    fn pipe_path_is_the_windows_form() {
        assert_eq!(pipe_path("synhand-excel"), r"\\.\pipe\synhand-excel");
    }

    #[test]
    fn invoke_maps_to_the_sidecar_method() {
        use crate::ops::StructArgs;
        let (mut h, w) = hand("{\"ok\":true,\"preview\":\"invoked Seven\"}
");
        let c = Call::Struct(StructArgs::Invoke { selector: "id=num7Button".into(), action: "invoke".into() });
        assert_eq!(h.dispatch(&c, "ui:Calculator::self").unwrap().preview, "invoked Seven");
        let out = sent(&w);
        assert!(out.contains("\"method\":\"invoke\""));
        assert!(out.contains("\"selector\":\"id=num7Button\""));
        assert!(out.contains("\"action\":\"invoke\""));
        assert!(out.contains("\"handle\":\"ui:Calculator::self\""));
    }

    #[test]
    fn the_analyst_verbs_reach_the_wire() {
        use crate::ops::{FormatArgs, StructArgs};
        // An analyst job needs a sheet, a style, a summary and a picture.
        // Each must arrive as its own method rather than being refused.
        let reply = "{\"ok\":true,\"preview\":\"done\"}
".repeat(4);
        let (mut h, w) = hand(&reply);
        h.dispatch(&Call::Struct(StructArgs::AddSheet { name: "Summary".into() }), "excel:p.xlsx:S").unwrap();
        h.dispatch(
            &Call::Format(FormatArgs {
                selector: "Summary!A1:D1".into(),
                style: vec![("bold".into(), "1".into()), ("numberFormat".into(), "#,##0".into())],
            }),
            "excel:p.xlsx:S",
        )
        .unwrap();
        h.dispatch(
            &Call::Struct(StructArgs::Pivot {
                source: "data!A1:H99".into(),
                rows: "name".into(),
                cols: String::new(),
                values: "kWh".into(),
                at: "Summary!F1".into(),
            }),
            "excel:p.xlsx:S",
        )
        .unwrap();
        h.dispatch(
            &Call::Struct(StructArgs::Chart {
                kind: "line".into(),
                source: "Summary!A1:B13".into(),
                title: "Monthly".into(),
                at: "Dashboard!A1:H16".into(),
                style: "legend=0;yTitle=kWh".into(),
            }),
            "excel:p.xlsx:S",
        )
        .unwrap();
        let out = sent(&w);
        for method in ["addSheet", "format", "pivot", "chart"] {
            assert!(out.contains(&format!("\"method\":\"{method}\"")), "{method} never reached the wire: {out}");
        }
        // The style rides as a payload in the shape the tool takes, so the
        // sidecar needs no second grammar for it.
        assert!(out.contains("bold=1;numberFormat=#,##0"), "{out}");
        assert!(out.contains("\"values\":\"kWh\""), "{out}");
        assert!(out.contains("\"kind\":\"line\""), "{out}");
        // A chart anchored to a range is how the model controls size, and the
        // style rides as a payload rather than as a second grammar.
        assert!(out.contains("\"at\":\"Dashboard!A1:H16\""), "{out}");
        assert!(out.contains("legend=0;yTitle=kWh"), "{out}");
    }
}
