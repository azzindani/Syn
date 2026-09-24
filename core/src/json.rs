//! JSON values, parsed and written with std alone.
//!
//! Everything else in `core` reads JSON with `tools::field`, a substring
//! search that is right for the flat objects a chat provider sends and
//! wrong the moment a message nests. An MCP request nests by design: the
//! client names itself in `params._meta.clientInfo.name`, and a search for
//! `"name"` finds that before it finds the tool. A server that dispatched on
//! it would run whatever the client happened to be called. So the one
//! component that speaks a nested protocol parses it properly.
//!
//! Small on purpose: objects keep their key order (a list of pairs, not a
//! map) so what is written back is stable, numbers keep their source text
//! so nothing is rounded on the way through, and there is a depth limit so
//! a hostile line cannot recurse the process off its stack.

use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// The number as it was written: `1`, `-2.5e3`. Kept as text so an id
    /// of `18446744073709551615` echoes back exactly.
    Num(String),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

/// Deeper than any real MCP message, shallow enough that a line of ten
/// thousand `[` is refused instead of overflowing the stack.
const MAX_DEPTH: usize = 64;

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Follow a path of keys: `v.at(&["params", "_meta", "x"])`.
    pub fn at(&self, path: &[&str]) -> Option<&Value> {
        path.iter().try_fold(self, |v, k| v.get(k))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_obj(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Obj(kv) => Some(kv),
            _ => None,
        }
    }

    pub fn as_arr(&self) -> Option<&[Value]> {
        match self {
            Value::Arr(a) => Some(a),
            _ => None,
        }
    }

    /// Serialise on one line: MCP's stdio framing forbids embedded newlines,
    /// and escaping them here is what guarantees a message never has one.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Num(n) => out.push_str(n),
            Value::Str(s) => write_str(s, out),
            Value::Arr(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write(out);
                }
                out.push(']');
            }
            Value::Obj(kv) => {
                out.push('{');
                for (i, (k, v)) in kv.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_str(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

/// Shorthand for building a response.
pub fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

pub fn s(text: impl Into<String>) -> Value {
    Value::Str(text.into())
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // U+2028/2029 are legal in JSON but end a line in some readers,
            // and a line break inside a stdio message ends the message.
            '\u{2028}' | '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Parse one complete JSON text. Trailing content is an error: a line that
/// is two messages glued together is not one message.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut p = Parser { s: text.as_bytes(), i: 0, src: text };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i != p.s.len() {
        return Err(format!("unexpected trailing text at byte {}", p.i));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    src: &'a str,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn eat(&mut self, lit: &str) -> bool {
        if self.s[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err("nested too deeply".into());
        }
        match self.s.get(self.i) {
            None => Err("unexpected end of input".into()),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Value::Str),
            Some(b't') if self.eat("true") => Ok(Value::Bool(true)),
            Some(b'f') if self.eat("false") => Ok(Value::Bool(false)),
            Some(b'n') if self.eat("null") => Ok(Value::Null),
            Some(c) if *c == b'-' || c.is_ascii_digit() => self.number(),
            Some(c) => Err(format!("unexpected {:?} at byte {}", *c as char, self.i)),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, String> {
        self.i += 1;
        let mut kv = Vec::new();
        self.ws();
        if self.eat("}") {
            return Ok(Value::Obj(kv));
        }
        loop {
            self.ws();
            if self.s.get(self.i) != Some(&b'"') {
                return Err(format!("expected a key at byte {}", self.i));
            }
            let k = self.string()?;
            self.ws();
            if !self.eat(":") {
                return Err(format!("expected ':' at byte {}", self.i));
            }
            self.ws();
            let v = self.value(depth + 1)?;
            kv.push((k, v));
            self.ws();
            if self.eat(",") {
                continue;
            }
            if self.eat("}") {
                return Ok(Value::Obj(kv));
            }
            return Err(format!("expected ',' or '}}' at byte {}", self.i));
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, String> {
        self.i += 1;
        let mut a = Vec::new();
        self.ws();
        if self.eat("]") {
            return Ok(Value::Arr(a));
        }
        loop {
            self.ws();
            a.push(self.value(depth + 1)?);
            self.ws();
            if self.eat(",") {
                continue;
            }
            if self.eat("]") {
                return Ok(Value::Arr(a));
            }
            return Err(format!("expected ',' or ']' at byte {}", self.i));
        }
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.i;
        if self.s.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        let digits = |p: &mut Self| {
            let from = p.i;
            while p.i < p.s.len() && p.s[p.i].is_ascii_digit() {
                p.i += 1;
            }
            p.i > from
        };
        if !digits(self) {
            return Err(format!("bad number at byte {start}"));
        }
        if self.eat(".") && !digits(self) {
            return Err(format!("bad number at byte {start}"));
        }
        if matches!(self.s.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.s.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if !digits(self) {
                return Err(format!("bad number at byte {start}"));
            }
        }
        Ok(Value::Num(self.src[start..self.i].to_string()))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let h = self.src.get(self.i..self.i + 4).ok_or("short \\u escape")?;
        self.i += 4;
        u32::from_str_radix(h, 16).map_err(|_| format!("bad \\u escape {h:?}"))
    }

    fn string(&mut self) -> Result<String, String> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let start = self.i;
            while self.i < self.s.len() && !matches!(self.s[self.i], b'"' | b'\\') {
                if self.s[self.i] < 0x20 {
                    return Err(format!("raw control character in a string at byte {}", self.i));
                }
                self.i += 1;
            }
            // The input is a &str, and the scan above only stops on ASCII,
            // so this slice is always on a char boundary.
            out.push_str(&self.src[start..self.i]);
            match self.s.get(self.i) {
                None => return Err("unterminated string".into()),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                _ => {
                    self.i += 1;
                    let e = *self.s.get(self.i).ok_or("unterminated escape")?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let c = if (0xd800..0xdc00).contains(&hi) && self.eat("\\u") {
                                let lo = self.hex4()?;
                                char::from_u32(0x10000 + ((hi - 0xd800) << 10) + (lo.wrapping_sub(0xdc00) & 0x3ff))
                            } else {
                                char::from_u32(hi)
                            };
                            out.push(c.unwrap_or('\u{fffd}'));
                        }
                        other => return Err(format!("bad escape \\{}", other as char)),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nested_name_is_not_mistaken_for_the_tool() {
        // The reason this module exists.
        let line = r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"_meta":{"io.modelcontextprotocol/clientInfo":{"name":"rm-rf"}},"name":"read","arguments":{"handle":"excel:a.xlsx:w","selector":"S!A1"}}}"#;
        let v = parse(line).unwrap();
        assert_eq!(v.at(&["params", "name"]).and_then(Value::as_str), Some("read"));
        assert_eq!(crate::tools::field(line, "name").as_deref(), Some("rm-rf"), "the substring reader gets this wrong");
    }

    #[test]
    fn round_trips_keep_order_numbers_and_escapes() {
        let src = r#"{"b":1,"a":[true,false,null,-2.5e3,"x\"y\\z\né😀"],"id":18446744073709551615}"#;
        let v = parse(src).unwrap();
        let back = v.to_json();
        assert_eq!(parse(&back).unwrap(), v);
        assert!(back.starts_with(r#"{"b":1,"a":"#), "key order kept: {back}");
        assert!(back.contains("18446744073709551615"), "a big id is echoed exactly");
        assert_eq!(v.at(&["a"]).and_then(Value::as_arr).map(|a| a[4].clone()), Some(Value::Str("x\"y\\z\né😀".into())));
    }

    #[test]
    fn output_never_contains_a_raw_line_break() {
        let v = obj(vec![("t", s("one\ntwo\r\u{2028}three"))]);
        let j = v.to_json();
        assert!(!j.contains('\n') && !j.contains('\r') && !j.contains('\u{2028}'), "{j}");
    }

    #[test]
    fn broken_input_is_refused_with_a_reason() {
        for bad in ["", "{", "{\"a\"}", "[1,]", "{\"a\":1}x", "\"raw\nnewline\"", "01x", "tru", "{\"a\":-}"] {
            assert!(parse(bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn a_hostile_depth_is_refused_not_recursed() {
        let deep = "[".repeat(10_000);
        assert!(parse(&deep).unwrap_err().contains("deeply"));
    }
}
