//! A streamed chat completion, folded back into one reply.
//!
//! The loop used to wait for a whole completion before showing anything,
//! behind `curl -m 60`: a model thinking hard for more than a minute was
//! cut off mid-thought and reported as "no HTTP response", and a person
//! watching saw nothing at all until the last token arrived. Streaming fixes
//! both. The provider sends the reply as it is written, a timeout can be
//! about silence rather than length, and the text can be shown as it grows.
//!
//! Everything downstream -- tool-call parsing, the empty-turn diagnosis, the
//! error-inside-a-200 check, the retry rules -- reads a non-streamed reply,
//! and was tested against real ones. So the stream is folded back into
//! exactly that shape (`finish`), and none of it had to change. What
//! arrives that is not a stream (a provider that ignores `"stream":true`,
//! or an error body) is passed through untouched.
//!
//! Deltas are handed to a callback as they arrive; the transport batches
//! them (`Throttle`) so a fast model does not become a line per token.

use crate::json::{self, Value};
use std::time::{Duration, Instant};

/// What a delta is: the reply itself, or the model's reasoning on the way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Thinking,
}

#[derive(Debug, Default, Clone)]
struct CallAcc {
    index: Option<usize>,
    id: String,
    name: String,
    args: String,
}

/// The reply so far.
#[derive(Debug, Default)]
pub struct Fold {
    saw_sse: bool,
    content: String,
    reasoning: String,
    calls: Vec<CallAcc>,
    finish: Option<String>,
    error: Option<Value>,
    id: Option<String>,
    model: Option<String>,
    usage: Option<Value>,
    /// Lines that were not a stream, verbatim: a plain JSON reply or an
    /// error body.
    other: String,
}

impl Fold {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether anything so far has been a stream.
    pub fn streamed(&self) -> bool {
        self.saw_sse
    }

    /// Take one line of the response.
    pub fn line(&mut self, line: &str, on: &mut dyn FnMut(Kind, &str)) {
        let t = line.trim_end_matches(['\r', '\n']);
        if let Some(p) = t.strip_prefix("data:") {
            self.saw_sse = true;
            let p = p.trim();
            if p.is_empty() || p == "[DONE]" {
                return;
            }
            // A chunk that does not parse is skipped, not fatal: one bad
            // line must not lose a reply that is otherwise fine.
            if let Ok(v) = json::parse(p) {
                self.chunk(&v, on);
            }
            return;
        }
        // Comments are keep-alives (": OPENROUTER PROCESSING" arrives every
        // few seconds while a model thinks); the other fields are framing.
        if t.starts_with(':') || (self.saw_sse && (t.is_empty() || t.starts_with("event:") || t.starts_with("id:") || t.starts_with("retry:"))) {
            return;
        }
        self.other.push_str(t);
        self.other.push('\n');
    }

    fn chunk(&mut self, v: &Value, on: &mut dyn FnMut(Kind, &str)) {
        if let Some(e) = v.get("error") {
            self.error = Some(e.clone());
            return;
        }
        if self.id.is_none() {
            self.id = v.get("id").and_then(Value::as_str).map(str::to_string);
        }
        if self.model.is_none() {
            self.model = v.get("model").and_then(Value::as_str).map(str::to_string);
        }
        if let Some(u) = v.get("usage").filter(|u| !matches!(u, Value::Null)) {
            self.usage = Some(u.clone());
        }
        let Some(choice) = v.get("choices").and_then(Value::as_arr).and_then(|c| c.first()) else { return };
        // `delta` is the stream's field; a few providers send their last
        // chunk as a whole `message`, which reads the same way.
        if let Some(d) = choice.get("delta").or_else(|| choice.get("message")) {
            if let Some(t) = d.get("content").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                self.content.push_str(t);
                on(Kind::Text, t);
            }
            // OpenRouter says `reasoning`; others `reasoning_content`.
            if let Some(t) = d.get("reasoning").or_else(|| d.get("reasoning_content")).and_then(Value::as_str).filter(|t| !t.is_empty()) {
                self.reasoning.push_str(t);
                on(Kind::Thinking, t);
            }
            if let Some(calls) = d.get("tool_calls").and_then(Value::as_arr) {
                for c in calls {
                    self.call_part(c);
                }
            }
        }
        if let Some(f) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish = Some(f.to_string());
        }
    }

    /// One fragment of one tool call. The id and name come in the first
    /// fragment and the arguments arrive in pieces after it, matched by
    /// `index`; without an index, by id; without either, it continues the
    /// last call.
    fn call_part(&mut self, c: &Value) {
        let index = c.get("index").and_then(|i| match i {
            Value::Num(n) => n.parse::<usize>().ok(),
            _ => None,
        });
        let id = c.get("id").and_then(Value::as_str).filter(|s| !s.is_empty());
        let slot = match (index, id) {
            (Some(i), _) => self.calls.iter().position(|a| a.index == Some(i)),
            (None, Some(id)) => self.calls.iter().position(|a| a.id == id),
            (None, None) => self.calls.len().checked_sub(1),
        };
        let slot = match slot {
            Some(s) => s,
            None => {
                self.calls.push(CallAcc { index, ..Default::default() });
                self.calls.len() - 1
            }
        };
        let acc = &mut self.calls[slot];
        if let Some(id) = id {
            acc.id = id.to_string();
        }
        if let Some(f) = c.get("function") {
            if let Some(n) = f.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()) {
                // Some providers repeat the whole name in every fragment.
                if acc.name.is_empty() {
                    acc.name = n.to_string();
                } else if acc.name != n {
                    acc.name.push_str(n);
                }
            }
            if let Some(a) = f.get("arguments").and_then(Value::as_str) {
                acc.args.push_str(a);
            }
        }
    }

    /// The reply, in the shape a non-streamed completion has.
    pub fn finish(self) -> String {
        if !self.saw_sse {
            return self.other.trim_end().to_string();
        }
        let mut message = vec![
            ("role", json::s("assistant")),
            ("content", if self.content.is_empty() { Value::Null } else { json::s(self.content) }),
        ];
        if !self.reasoning.is_empty() {
            message.push(("reasoning", json::s(self.reasoning)));
        }
        let calls: Vec<Value> = self
            .calls
            .into_iter()
            .filter(|c| !c.name.is_empty())
            .enumerate()
            .map(|(i, c)| {
                json::obj(vec![
                    ("id", json::s(if c.id.is_empty() { format!("call_{}", i + 1) } else { c.id })),
                    ("type", json::s("function")),
                    (
                        "function",
                        json::obj(vec![
                            ("name", json::s(c.name)),
                            ("arguments", json::s(if c.args.trim().is_empty() { "{}".to_string() } else { c.args })),
                        ]),
                    ),
                ])
            })
            .collect();
        if !calls.is_empty() {
            message.push(("tool_calls", Value::Arr(calls)));
        }
        let mut body = Vec::new();
        // An error goes first: `error_in_ok_body` tells an error envelope
        // from a reply that mentions one by which key comes first.
        if let Some(e) = self.error {
            body.push(("error", e));
        }
        if let Some(id) = self.id {
            body.push(("id", json::s(id)));
        }
        if let Some(m) = self.model {
            body.push(("model", json::s(m)));
        }
        body.push((
            "choices",
            Value::Arr(vec![json::obj(vec![
                ("index", Value::Num("0".into())),
                ("finish_reason", self.finish.map(json::s).unwrap_or(Value::Null)),
                ("message", json::obj(message)),
            ])]),
        ));
        if let Some(u) = self.usage {
            body.push(("usage", u));
        }
        json::obj(body).to_json()
    }
}

/// `"stream":true` added to a request body that does not say either way.
pub fn with_stream_flag(body: &str) -> String {
    let b = body.trim_end();
    if b.contains("\"stream\":") || !b.ends_with('}') {
        return b.to_string();
    }
    format!("{},\"stream\":true}}", &b[..b.len() - 1])
}

/// Batches deltas so a fast model is a few updates a second, not one per
/// token: flushed when a kind has gathered enough text or waited long
/// enough, and on `flush`.
pub struct Throttle<'a> {
    out: &'a mut dyn FnMut(Kind, &str),
    text: String,
    thinking: String,
    last: Instant,
    every: Duration,
    size: usize,
}

impl<'a> Throttle<'a> {
    pub fn new(out: &'a mut dyn FnMut(Kind, &str)) -> Self {
        Self { out, text: String::new(), thinking: String::new(), last: Instant::now(), every: Duration::from_millis(120), size: 160 }
    }

    pub fn push(&mut self, kind: Kind, t: &str) {
        match kind {
            Kind::Text => self.text.push_str(t),
            Kind::Thinking => self.thinking.push_str(t),
        }
        if self.text.len() + self.thinking.len() >= self.size || self.last.elapsed() >= self.every {
            self.flush();
        }
    }

    /// Flush if the pending text has waited long enough. Called between
    /// lines, so a model that pauses mid-sentence still shows what it has.
    pub fn tick(&mut self) {
        if self.last.elapsed() >= self.every {
            self.flush();
        }
    }

    pub fn flush(&mut self) {
        // Thinking before text: when both are pending, the reasoning came
        // first.
        if !self.thinking.is_empty() {
            (self.out)(Kind::Thinking, &std::mem::take(&mut self.thinking));
        }
        if !self.text.is_empty() {
            (self.out)(Kind::Text, &std::mem::take(&mut self.text));
        }
        self.last = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold(lines: &[&str]) -> (String, Vec<(Kind, String)>) {
        let mut seen = Vec::new();
        let mut f = Fold::new();
        for l in lines {
            f.line(l, &mut |k, t| seen.push((k, t.to_string())));
        }
        (f.finish(), seen)
    }

    #[test]
    fn a_streamed_answer_reads_back_as_the_answer() {
        let (body, seen) = fold(&[
            ": OPENROUTER PROCESSING",
            "",
            r#"data: {"id":"gen-1","model":"m","choices":[{"index":0,"delta":{"role":"assistant","content":"Hel"}}]}"#,
            "",
            r#"data: {"id":"gen-1","choices":[{"index":0,"delta":{"content":"lo \"you\""},"finish_reason":"stop"}]}"#,
            "data: [DONE]",
        ]);
        assert_eq!(crate::provider::parse_chat_text(&body).as_deref(), Some("Hello \"you\""));
        assert!(body.contains(r#""finish_reason":"stop""#), "{body}");
        assert_eq!(seen, vec![(Kind::Text, "Hel".into()), (Kind::Text, "lo \"you\"".into())]);
    }

    #[test]
    fn tool_calls_arriving_in_pieces_come_out_whole() {
        // Two calls in one turn, each split across chunks, interleaved by
        // index the way parallel calls really stream.
        let (body, _) = fold(&[
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"read","arguments":""}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"c2","type":"function","function":{"name":"write","arguments":"{\"handle\":"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"handle\":\"h\",\"sel"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ector\":\"A1\"}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"arguments":"\"h\",\"selector\":\"B1\",\"values\":\"x\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            "data: [DONE]",
        ]);
        let calls = crate::tools::parse_tool_calls(&body);
        assert_eq!(calls.len(), 2, "{body}");
        assert_eq!((calls[0].id.as_str(), calls[0].name.as_str()), ("c1", "read"));
        assert_eq!(calls[0].arguments, r#"{"handle":"h","selector":"A1"}"#);
        assert_eq!(calls[1].arguments, r#"{"handle":"h","selector":"B1","values":"x"}"#);
        assert!(crate::tools::raw_tool_calls(&body).is_some(), "the assistant turn can be echoed back");
    }

    #[test]
    fn reasoning_is_shown_as_thinking_and_kept_out_of_the_answer() {
        let (body, seen) = fold(&[
            r#"data: {"choices":[{"delta":{"reasoning":"Let me look"}}]}"#,
            r#"data: {"choices":[{"delta":{"reasoning_content":" at A1."}}]}"#,
            r#"data: {"choices":[{"delta":{"content":"Done."},"finish_reason":"stop"}]}"#,
        ]);
        assert_eq!(crate::provider::parse_chat_text(&body).as_deref(), Some("Done."));
        assert!(body.contains(r#""reasoning":"Let me look at A1.""#), "{body}");
        assert_eq!(seen[0], (Kind::Thinking, "Let me look".into()));
    }

    #[test]
    fn an_error_inside_the_stream_is_reported_like_one_inside_a_200() {
        let (body, _) = fold(&[
            r#"data: {"choices":[{"delta":{"content":"partial"}}]}"#,
            r#"data: {"error":{"message":"Upstream error from Nvidia: Service temporarily overloaded","code":503,"metadata":{"error_type":"provider_overloaded"}}}"#,
        ]);
        let e = crate::provider::error_in_ok_body(&body).expect("the error is seen");
        assert!(e.contains("overloaded"), "{e}");
    }

    #[test]
    fn a_provider_that_does_not_stream_is_passed_through_untouched() {
        let plain = r#"{"choices":[{"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}]}"#;
        let (body, seen) = fold(&[plain]);
        assert_eq!(body, plain);
        assert!(seen.is_empty());
        let err = r#"{"error":{"message":"No auth credentials found","code":401}}"#;
        assert_eq!(fold(&[err]).0, err);
    }

    #[test]
    fn a_turn_that_says_nothing_still_looks_like_an_empty_turn() {
        let (body, _) = fold(&[r#"data: {"choices":[{"delta":{},"finish_reason":"length"}]}"#]);
        assert!(crate::provider::no_completion_in_ok_body(&body).is_some() || crate::provider::empty_turn_diagnosis(&body).contains("length"), "{body}");
    }

    #[test]
    fn a_request_is_marked_for_streaming_once() {
        assert_eq!(with_stream_flag(r#"{"model":"m"}"#), r#"{"model":"m","stream":true}"#);
        assert_eq!(with_stream_flag(r#"{"model":"m","stream":false}"#), r#"{"model":"m","stream":false}"#);
    }

    #[test]
    fn deltas_are_batched_not_sent_a_token_at_a_time() {
        let mut got: Vec<(Kind, String)> = Vec::new();
        {
            let mut sink = |k: Kind, t: &str| got.push((k, t.to_string()));
            let mut th = Throttle::new(&mut sink);
            for w in ["a", "b", "c", "d"] {
                th.push(Kind::Text, w);
            }
            th.flush();
        }
        assert_eq!(got, vec![(Kind::Text, "abcd".to_string())]);
    }
}
