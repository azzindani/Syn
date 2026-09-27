//! OpenAI's Responses API, spoken at the wire.
//!
//! Syn builds chat-completions requests and reads chat-completions replies
//! everywhere above the transport. Some models are served only on
//! `/responses`: OpenCode Go answered "Model does not support this protocol"
//! for Muse Spark on `/chat/completions`, and the same request on
//! `/responses` came back. Rather than teach the loop a second vocabulary,
//! the request is translated on its way out and the reply on its way back,
//! so everything that parses a reply keeps reading one shape.
//!
//!   chat                               responses
//!   system / user / assistant text  -> an input message with that role
//!   assistant tool_calls            -> function_call items (call_id kept)
//!   tool result                     -> function_call_output item
//!   tools[].function                -> tools[] flattened
//!   reasoning / reasoning_effort    -> reasoning.effort
//!
//! and back: output_text becomes the message content, function_call items
//! become tool_calls, reasoning summaries become `reasoning`. `store` is off:
//! the whole conversation travels with every request, as it does on the
//! chat API, and nothing is kept on the provider's side.

use crate::json::{self, Value};
use crate::sse::Kind;

/// A chat-completions request body as a Responses request body.
pub fn to_request(chat: &str) -> Result<String, String> {
    let v = json::parse(chat).map_err(|e| format!("responses: the request is not JSON: {e}"))?;
    let mut input = Vec::new();
    for m in v.get("messages").and_then(Value::as_arr).unwrap_or(&[]) {
        let role = m.get("role").and_then(Value::as_str).unwrap_or("");
        let text = m.get("content").and_then(Value::as_str).filter(|t| !t.is_empty());
        match role {
            "system" | "user" | "assistant" => {
                if let Some(t) = text {
                    input.push(json::obj(vec![("role", json::s(role)), ("content", json::s(t))]));
                }
                for c in m.get("tool_calls").and_then(Value::as_arr).unwrap_or(&[]) {
                    input.push(json::obj(vec![
                        ("type", json::s("function_call")),
                        ("call_id", c.get("id").cloned().unwrap_or_else(|| json::s(""))),
                        ("name", c.at(&["function", "name"]).cloned().unwrap_or_else(|| json::s(""))),
                        ("arguments", c.at(&["function", "arguments"]).cloned().unwrap_or_else(|| json::s("{}"))),
                    ]));
                }
            }
            "tool" => input.push(json::obj(vec![
                ("type", json::s("function_call_output")),
                ("call_id", m.get("tool_call_id").cloned().unwrap_or_else(|| json::s(""))),
                ("output", json::s(text.unwrap_or(""))),
            ])),
            _ => {}
        }
    }
    let mut out = vec![("model", v.get("model").cloned().unwrap_or(Value::Null)), ("input", Value::Arr(input))];
    if let Some(tools) = v.get("tools").and_then(Value::as_arr) {
        let flat: Vec<Value> = tools
            .iter()
            .filter_map(|t| {
                let f = t.get("function")?;
                let mut kv = vec![("type", json::s("function")), ("name", f.get("name")?.clone())];
                if let Some(d) = f.get("description") {
                    kv.push(("description", d.clone()));
                }
                if let Some(p) = f.get("parameters") {
                    kv.push(("parameters", p.clone()));
                }
                Some(json::obj(kv))
            })
            .collect();
        out.push(("tools", Value::Arr(flat)));
        out.push(("tool_choice", json::s("auto")));
    }
    if let Some(e) = v.at(&["reasoning", "effort"]).or_else(|| v.get("reasoning_effort")) {
        out.push(("reasoning", json::obj(vec![("effort", e.clone())])));
    }
    if let Some(s) = v.get("stream") {
        out.push(("stream", s.clone()));
    }
    out.push(("store", Value::Bool(false)));
    Ok(json::obj(out).to_json())
}

/// One finished Responses object in the chat-completions shape.
fn from_response(r: &Value) -> Value {
    let (mut text, mut reasoning, mut calls) = (String::new(), String::new(), Vec::new());
    for item in r.get("output").and_then(Value::as_arr).unwrap_or(&[]) {
        match item.get("type").and_then(Value::as_str).unwrap_or("") {
            "message" => {
                for c in item.get("content").and_then(Value::as_arr).unwrap_or(&[]) {
                    if c.get("type").and_then(Value::as_str) == Some("output_text")
                        && let Some(t) = c.get("text").and_then(Value::as_str)
                    {
                        text.push_str(t);
                    }
                }
            }
            "reasoning" => {
                for part in ["summary", "content"] {
                    for c in item.get(part).and_then(Value::as_arr).unwrap_or(&[]) {
                        if let Some(t) = c.get("text").and_then(Value::as_str) {
                            reasoning.push_str(t);
                        }
                    }
                }
            }
            "function_call" => calls.push(json::obj(vec![
                ("id", item.get("call_id").or_else(|| item.get("id")).cloned().unwrap_or_else(|| json::s(""))),
                ("type", json::s("function")),
                (
                    "function",
                    json::obj(vec![
                        ("name", item.get("name").cloned().unwrap_or_else(|| json::s(""))),
                        ("arguments", item.get("arguments").cloned().unwrap_or_else(|| json::s("{}"))),
                    ]),
                ),
            ])),
            _ => {}
        }
    }
    let finish = if !calls.is_empty() {
        "tool_calls"
    } else if r.at(&["incomplete_details", "reason"]).and_then(Value::as_str) == Some("max_output_tokens") {
        "length"
    } else {
        "stop"
    };
    let mut message = vec![
        ("role", json::s("assistant")),
        ("content", if text.is_empty() { Value::Null } else { json::s(text) }),
    ];
    if !reasoning.is_empty() {
        message.push(("reasoning", json::s(reasoning)));
    }
    if !calls.is_empty() {
        message.push(("tool_calls", Value::Arr(calls)));
    }
    let mut body = Vec::new();
    if let Some(e) = r.get("error").filter(|e| !matches!(e, Value::Null)) {
        body.push(("error", e.clone()));
    }
    if let Some(id) = r.get("id") {
        body.push(("id", id.clone()));
    }
    if let Some(m) = r.get("model") {
        body.push(("model", m.clone()));
    }
    body.push((
        "choices",
        Value::Arr(vec![json::obj(vec![
            ("index", Value::Num("0".into())),
            ("finish_reason", json::s(finish)),
            ("message", json::obj(message)),
        ])]),
    ));
    if let Some(u) = r.get("usage") {
        let n = |k: &str| u.get(k).cloned().unwrap_or(Value::Num("0".into()));
        body.push((
            "usage",
            json::obj(vec![("prompt_tokens", n("input_tokens")), ("completion_tokens", n("output_tokens")), ("total_tokens", n("total_tokens"))]),
        ));
    }
    json::obj(body)
}

/// A whole (not streamed) Responses reply in the chat shape. An error
/// envelope, or anything that is not a response, is passed on as it came,
/// so the error reporting above reads it unchanged.
pub fn to_chat_reply(body: &str) -> String {
    match json::parse(body.trim()) {
        Ok(v) if v.get("output").is_some() => from_response(&v).to_json(),
        _ => body.to_string(),
    }
}

/// A streamed Responses reply, folded as it arrives: text and reasoning
/// deltas go to `on` while the model writes, and the finished response --
/// which the last event carries whole -- becomes the reply.
#[derive(Default)]
pub struct StreamFold {
    saw_sse: bool,
    done: Option<Value>,
    error: Option<Value>,
    /// Lines that were not a stream: an error body.
    other: String,
}

impl StreamFold {
    pub fn line(&mut self, line: &str, on: &mut dyn FnMut(Kind, &str)) {
        let t = line.trim_end_matches(['\r', '\n']);
        if t.starts_with("event:") || t.is_empty() {
            return;
        }
        let Some(p) = t.strip_prefix("data:") else {
            self.other.push_str(t);
            self.other.push('\n');
            return;
        };
        self.saw_sse = true;
        let Ok(v) = json::parse(p.trim()) else { return };
        let delta = || v.get("delta").and_then(Value::as_str).unwrap_or("");
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "response.output_text.delta" => on(Kind::Text, delta()),
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => on(Kind::Thinking, delta()),
            "response.completed" | "response.incomplete" | "response.failed" => self.done = v.get("response").cloned(),
            "error" => self.error = Some(v.clone()),
            _ => {}
        }
    }

    pub fn finish(self) -> String {
        if let Some(r) = self.done {
            return from_response(&r).to_json();
        }
        if let Some(e) = self.error {
            let msg = e.get("message").or_else(|| e.at(&["error", "message"])).cloned().unwrap_or_else(|| json::s("the stream reported an error"));
            return json::obj(vec![("error", json::obj(vec![("message", msg)]))]).to_json();
        }
        if !self.saw_sse {
            return self.other.trim_end().to_string();
        }
        json::obj(vec![("error", json::obj(vec![("message", json::s("the stream ended before the response was complete"))]))]).to_json()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAT: &str = r#"{"model":"muse-spark-1.3-contributor","reasoning":{"effort":"medium"},"messages":[
        {"role":"system","content":"You drive apps."},
        {"role":"user","content":"read A1"},
        {"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"handle\":\"h\",\"selector\":\"A1\"}"}}]},
        {"role":"tool","tool_call_id":"call_1","content":"grid 1x1 = name"},
        {"role":"assistant","content":"A1 says name."}],
        "tools":[{"type":"function","function":{"name":"read","description":"Read.","parameters":{"type":"object"}}}],"tool_choice":"auto","stream":true}"#;

    #[test]
    fn a_chat_request_becomes_the_same_conversation_as_responses_input() {
        let r = json::parse(&to_request(CHAT).unwrap()).unwrap();
        assert_eq!(r.get("model").and_then(Value::as_str), Some("muse-spark-1.3-contributor"));
        let input = r.get("input").and_then(Value::as_arr).unwrap();
        let kinds: Vec<String> = input
            .iter()
            .map(|i| i.get("type").and_then(Value::as_str).or(i.get("role").and_then(Value::as_str)).unwrap().to_string())
            .collect();
        assert_eq!(kinds, ["system", "user", "function_call", "function_call_output", "assistant"]);
        assert_eq!(input[2].get("call_id").and_then(Value::as_str), Some("call_1"), "the call and its result stay paired");
        assert_eq!(input[3].get("call_id").and_then(Value::as_str), Some("call_1"));
        assert_eq!(input[3].get("output").and_then(Value::as_str), Some("grid 1x1 = name"));
        let tool = &r.get("tools").and_then(Value::as_arr).unwrap()[0];
        assert_eq!((tool.get("type").and_then(Value::as_str), tool.get("name").and_then(Value::as_str)), (Some("function"), Some("read")));
        assert_eq!(r.at(&["reasoning", "effort"]).and_then(Value::as_str), Some("medium"));
        assert_eq!(r.get("stream"), Some(&Value::Bool(true)));
        assert_eq!(r.get("store"), Some(&Value::Bool(false)), "nothing kept on the provider's side");
        // The flat spelling other providers get is read too.
        let flat = to_request(r#"{"model":"m","reasoning_effort":"high","messages":[]}"#).unwrap();
        assert!(flat.contains(r#""reasoning":{"effort":"high"}"#), "{flat}");
    }

    // Trimmed from Go's real reply for muse-spark-1.3-contributor.
    const DONE: &str = r#"{"id":"resp_1","object":"response","status":"completed","model":"muse-spark-1.3-contributor","error":null,
        "output":[{"type":"reasoning","summary":[{"type":"summary_text","text":"Need A1."}]},
                  {"type":"function_call","call_id":"call_9","name":"read","arguments":"{\"selector\":\"A1\"}"}],
        "usage":{"input_tokens":13,"output_tokens":50,"total_tokens":63}}"#;

    #[test]
    fn a_response_comes_back_in_the_chat_shape_the_loop_reads() {
        let chat = to_chat_reply(DONE);
        let calls = crate::tools::parse_tool_calls(&chat);
        assert_eq!(calls.len(), 1, "{chat}");
        assert_eq!((calls[0].id.as_str(), calls[0].name.as_str()), ("call_9", "read"));
        let v = json::parse(&chat).unwrap();
        assert_eq!(v.at(&["choices"]).and_then(Value::as_arr).unwrap()[0].get("finish_reason").and_then(Value::as_str), Some("tool_calls"));
        assert_eq!(v.at(&["usage", "prompt_tokens"]), Some(&Value::Num("13".into())));
        let text = to_chat_reply(r#"{"id":"r","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"pineapple"}]}]}"#);
        assert_eq!(crate::provider::parse_chat_text(&text).as_deref(), Some("pineapple"));
        // An error envelope is passed on untouched.
        let err = r#"{"type":"error","error":{"message":"Missing API key."}}"#;
        assert_eq!(to_chat_reply(err), err);
    }

    #[test]
    fn a_streamed_response_shows_its_words_as_they_come_and_ends_as_one_reply() {
        let mut seen = Vec::new();
        let mut f = StreamFold::default();
        for l in [
            "event: response.created",
            r#"data: {"type":"response.created","response":{"id":"r"}}"#,
            r#"data: {"type":"response.reasoning_summary_text.delta","delta":"thinking"}"#,
            r#"data: {"type":"response.output_text.delta","delta":"pine"}"#,
            r#"data: {"type":"response.output_text.delta","delta":"apple"}"#,
            r#"data: {"type":"response.completed","response":{"id":"r","status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"pineapple"}]}]}}"#,
        ] {
            f.line(l, &mut |k, t| seen.push((matches!(k, Kind::Text), t.to_string())));
        }
        assert_eq!(seen, [(false, "thinking".into()), (true, "pine".into()), (true, "apple".into())]);
        assert_eq!(crate::provider::parse_chat_text(&f.finish()).as_deref(), Some("pineapple"));
        // A plain error body, not a stream, comes back as it was.
        let mut e = StreamFold::default();
        e.line(r#"{"error":{"message":"nope"}}"#, &mut |_, _| {});
        assert_eq!(e.finish(), r#"{"error":{"message":"nope"}}"#);
    }
}
