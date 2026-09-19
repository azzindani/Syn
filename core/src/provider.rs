//! Provider client (OpenRouter-compatible): endpoint + request-body builder.
//! No network in core: the widget performs transport with a user-supplied key
//! (env `SYN_API_KEY`, never logged, never stored). Model IDs are defaults
//! overridable per deployment. Mock transport lives in tests.

use crate::router::{Effort, Model, Route};

/// Default model IDs. Override via deployment config, not code edits.
pub fn model_id(model: Model) -> &'static str {
    match model {
        Model::Luna => "openai/gpt-5.6-luna",
        Model::Terra => "openai/gpt-5.6-terra",
        Model::Sol => "openai/gpt-5.6-sol",
        Model::Astra => "openai/gpt-6-astra",
    }
}

pub fn effort_str(effort: Effort) -> &'static str {
    match effort {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Max => "max",
    }
}

pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
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

/// Minimal chat-completions body against an explicit model id. Callers that
/// honour deployment config resolve the id through `config::model_id` first;
/// `request_body` keeps the compiled default.
/// Std only (no serde in core); the widget may use a JSON library.
pub fn request_body_with(model: &str, route: Route, system: &str, user: &str) -> String {
    format!(
        "{{\"model\":\"{}\",\"reasoning\":{{\"effort\":\"{}\"}},\"messages\":[{{\"role\":\"system\",\"content\":\"{}\"}},{{\"role\":\"user\",\"content\":\"{}\"}}]}}",
        escape_json(model),
        effort_str(route.effort),
        escape_json(system),
        escape_json(user)
    )
}

/// Minimal chat-completions body: model + effort + system/user messages.
pub fn request_body(route: Route, system: &str, user: &str) -> String {
    request_body_with(model_id(route.model), route, system, user)
}

/// Streaming variant of [`request_body_with`]: plus `"stream":true` for SSE.
pub fn stream_body_with(model: &str, route: Route, system: &str, user: &str) -> String {
    let mut b = request_body_with(model, route, system, user);
    b.pop(); // drop closing '}'
    b.push_str(r#","stream":true}"#);
    b
}

/// Streaming variant: identical body plus `"stream":true` for SSE.
/// The widget reads `text/event-stream` chunks; core parses them below.
pub fn stream_body(route: Route, system: &str, user: &str) -> String {
    stream_body_with(model_id(route.model), route, system, user)
}

/// One turn in the conversation the agent loop replays each step.
///
/// `AssistantCalls` carries the provider's own `tool_calls` array verbatim:
/// the chat protocol rejects a tool result unless the assistant turn that
/// requested it is echoed back exactly, so re-serialising a parsed form
/// would risk a mismatch the server refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    System(String),
    User(String),
    Assistant(String),
    AssistantCalls(String),
    Tool { id: String, content: String },
}

impl Msg {
    fn render(&self) -> String {
        match self {
            Msg::System(c) => format!(r#"{{"role":"system","content":"{}"}}"#, escape_json(c)),
            Msg::User(c) => format!(r#"{{"role":"user","content":"{}"}}"#, escape_json(c)),
            Msg::Assistant(c) => format!(r#"{{"role":"assistant","content":"{}"}}"#, escape_json(c)),
            Msg::AssistantCalls(raw) => {
                format!(r#"{{"role":"assistant","content":null,"tool_calls":{raw}}}"#)
            }
            Msg::Tool { id, content } => format!(
                r#"{{"role":"tool","tool_call_id":"{}","content":"{}"}}"#,
                escape_json(id),
                escape_json(content)
            ),
        }
    }
}

/// Full chat-completions body for the agent loop: whole message history plus
/// the tool surface. `tools` is the array text from `tools::tools_json`.
pub fn chat_body(model: &str, route: Route, msgs: &[Msg], tools: Option<&str>) -> String {
    let rendered: Vec<String> = msgs.iter().map(Msg::render).collect();
    let mut b = format!(
        r#"{{"model":"{}","reasoning":{{"effort":"{}"}},"messages":[{}]"#,
        escape_json(model),
        effort_str(route.effort),
        rendered.join(",")
    );
    if let Some(t) = tools {
        // "auto": the model may answer in prose instead of calling a tool,
        // which is how a turn ends.
        b.push_str(&format!(r#","tools":{t},"tool_choice":"auto""#));
    }
    b.push('}');
    b
}

/// Unescape the JSON string subset the API emits in deltas.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('u') => {
                    let hex: String = it.by_ref().take(4).collect();
                    out.push(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32).unwrap_or('�'));
                }
                Some(o) => out.push(o),
                None => break,
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Pull every `"content":"..."` value out of one JSON payload (std only).
fn extract_contents(json: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = json;
    while let Some(i) = rest.find("\"content\"") {
        rest = &rest[i + 9..];
        let rest2 = rest.trim_start_matches([' ', ':']);
        if !rest2.starts_with('"') {
            rest = rest2;
            continue;
        }
        let mut raw = String::new();
        let mut it = rest2[1..].chars();
        let mut closed = false;
        while let Some(c) = it.next() {
            if c == '\\' {
                raw.push('\\');
                if let Some(e) = it.next() {
                    raw.push(e);
                }
            } else if c == '"' {
                closed = true;
                break;
            } else {
                raw.push(c);
            }
        }
        if closed {
            out.push(unescape(&raw));
        }
        rest = it.as_str();
    }
    out
}

/// Parse one SSE chunk: concatenate `data:` payload contents.
/// Skips `data: [DONE]`, comments (`: ...`), and malformed lines.
pub fn parse_sse_text(chunk: &str) -> String {
    let mut text = String::new();
    for line in chunk.lines() {
        let line = line.trim();
        let Some(payload) = line.strip_prefix("data:") else { continue };
        let payload = payload.trim();
        if payload == "[DONE]" || payload.is_empty() {
            continue;
        }
        for c in extract_contents(payload) {
            text.push_str(&c);
        }
    }
    text
}

/// Parse a non-streaming chat-completions body: first message content.
pub fn parse_chat_text(body: &str) -> Option<String> {
    extract_contents(body).into_iter().next()
}

/// Transport is implemented widget-side; core only defines the seam.
pub trait Transport {
    fn post(&self, url: &str, api_key_env: &str, body: &str) -> Result<String, String>;
}

/// Model picker: router default, user override wins, budget latch stops
/// spend. cost_rank accumulates per call; cap is the session budget.
#[derive(Debug, Default)]
pub struct Picker {
    pub override_model: Option<Model>,
    pub spent: u32,
}

impl Picker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pick(&self, task: crate::router::TaskKind) -> Route {
        let mut r = crate::router::route(task);
        if let Some(m) = self.override_model {
            r.model = m;
        }
        r
    }

    pub fn note_spend(&mut self, route: Route) {
        self.spent += route.cost_rank as u32;
    }

    pub fn over_budget(&self, cap: u32) -> bool {
        self.spent >= cap
    }
}

/// Real send via the `curl` binary (core stays TLS-free).
/// Key is read from `api_key_env` at send time and never logged.
/// Returns (http_status, response_body).
pub fn send_via_curl(base_url: &str, api_key_env: &str, body: &str) -> Result<(u16, String), String> {
    let key = std::env::var(api_key_env).map_err(|_| format!("env {api_key_env} not set"))?;
    let url = format!("{base_url}/chat/completions");
    let out = std::process::Command::new("curl")
        .args(["-sS", "-m", "60", "-X", "POST", &url])
        .args(["-H", "Content-Type: application/json"])
        .args(["-H", &format!("Authorization: Bearer {key}")])
        .args(["--data-binary", "@-"])
        .arg("-w")
        .arg("\n%{http_code}")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl spawn failed: {e}"))?;
    use std::io::Write;
    let mut child = out;
    // A curl that died early (bad URL, TLS failure) closes stdin, so the
    // write fails with BrokenPipe. That is not the interesting error: the
    // reason is on stderr, so keep going and report that instead.
    if let Some(mut sink) = child.stdin.take()
        && let Err(e) = sink.write_all(body.as_bytes())
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(format!("stdin: {e}"));
    }
    let res = child.wait_with_output().map_err(|e| format!("curl wait: {e}"))?;
    let stderr = String::from_utf8_lossy(&res.stderr).trim().to_string();
    // Empty stdout means no reply at all, whatever the exit code claims.
    // The previous form required BOTH a bad exit code AND empty stdout, so
    // the common case — curl fails, says why on stderr, exits quietly —
    // was reported to the user as "status=0 bytes=0" with the reason thrown
    // away. Never discard stderr: it is the only diagnosis available.
    let text = String::from_utf8_lossy(&res.stdout);
    let (payload, code) = text.rsplit_once('\n').unwrap_or((&text, "0"));
    let code: u16 = code.trim().parse().unwrap_or(0);
    // http_code 000 is curl's way of saying no HTTP response happened at
    // all: DNS failure, refused connection, TLS error, timeout. curl still
    // writes that 000 to stdout, so testing for empty stdout is not enough
    // and the reason only ever exists on stderr.
    if code == 0 {
        return Err(if stderr.is_empty() {
            format!("curl produced no HTTP response ({})", res.status)
        } else {
            format!("curl transport failed: {stderr}")
        });
    }
    Ok((code, payload.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::{route, TaskKind};

    struct Mock {
        seen_url: std::cell::RefCell<String>,
        seen_body: std::cell::RefCell<String>,
    }

    impl Transport for Mock {
        fn post(&self, url: &str, _key: &str, body: &str) -> Result<String, String> {
            *self.seen_url.borrow_mut() = url.into();
            *self.seen_body.borrow_mut() = body.into();
            Ok(r#"{"choices":[]}"#.into())
        }
    }

    #[test]
    fn body_carries_model_effort_and_escaped_prompt() {
        let r = route(TaskKind::Code);
        let body = request_body(r, "sys", "say \"hi\"\nnewline");
        assert!(body.contains("openai/gpt-5.6-sol"));
        assert!(body.contains("\\\"hi\\\""));
        assert!(body.contains("\\n"));
        let m = Mock { seen_url: Default::default(), seen_body: Default::default() };
        m.post(DEFAULT_BASE_URL, "SYN_API_KEY", &body).unwrap();
        assert!(m.seen_url.borrow().contains("openrouter.ai"));
        assert!(m.seen_body.borrow().contains("gpt-5.6-sol"));
    }

    #[test]
    fn astra_route_resolves_flagship() {
        let r = route(TaskKind::VisionFallback);
        assert!(request_body(r, "", "").contains("openai/gpt-6-astra"));
    }

    #[test]
    fn stream_body_flags_sse() {
        let b = stream_body(route(TaskKind::Routine), "s", "u");
        assert!(b.contains(r#""stream":true"#));
        assert!(b.contains("gpt-5.6-terra"));
    }

    #[test]
    fn sse_parses_deltas_and_done() {
        let chunk = ": ping\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"lo \\\"you\\\"\"}}]}\n\ndata: [DONE]\n\ndata: not-json\n";
        assert_eq!(parse_sse_text(chunk), "Hello \"you\"");
    }

    #[test]
    fn chat_parse_reads_first_content() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"do the thing"}}]}"#;
        assert_eq!(parse_chat_text(body).as_deref(), Some("do the thing"));
        assert_eq!(parse_chat_text(r#"{"choices":[]}"#), None);
    }

    #[test]
    fn picker_override_and_budget() {
        use crate::router::TaskKind;
        let mut p = Picker::new();
        assert_eq!(p.pick(TaskKind::Skim).model, Model::Luna);
        p.override_model = Some(Model::Sol);
        let r = p.pick(TaskKind::Skim);
        assert_eq!(r.model, Model::Sol);
        p.note_spend(r);
        assert!(!p.over_budget(10));
        assert!(p.over_budget(1));
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;
    use crate::router::{TaskKind, route};

    #[test]
    fn sse_skips_noise_and_handles_unicode() {
        let chunk = "data: {\"choices\":[{\"delta\":{\"content\":\"caf\\u00e9\"}}]}\ndata:\n:comment\nnot-a-data-line\n";
        assert_eq!(parse_sse_text(chunk), "café");
        assert_eq!(parse_sse_text(""), "");
        assert_eq!(parse_sse_text("data: [DONE]\n"), "");
    }

    #[test]
    fn chat_parse_edge_cases() {
        assert_eq!(parse_chat_text(""), None);
        assert_eq!(parse_chat_text("no content key here"), None);
        // Multiple contents: first wins (assistant message before tool calls).
        let b = r#"{"choices":[{"message":{"content":"first"}},{"delta":{"content":"second"}}]}"#;
        assert_eq!(parse_chat_text(b).as_deref(), Some("first"));
    }

    #[test]
    fn request_body_escapes_backslash_and_control() {
        let b = request_body(route(TaskKind::Code), "s", "a\\b\x01c");
        assert!(b.contains("a\\\\b"));
        assert!(b.contains("\\u0001"));
    }

    #[test]
    fn all_task_kinds_have_ids() {
        for t in [TaskKind::Skim, TaskKind::Routine, TaskKind::Code, TaskKind::DeepReasoning, TaskKind::VisionFallback] {
            let id = model_id(route(t).model);
            assert!(id.contains('/'), "{t:?}");
            assert!(effort_str(route(t).effort).len() >= 3);
        }
    }

    #[test]
    fn picker_budget_accumulates() {
        let mut p = Picker::default();
        assert!(!p.over_budget(1)); // spent 0, cap 1: not over yet
        p.note_spend(route(TaskKind::Routine)); // +2
        p.note_spend(route(TaskKind::Skim)); // +1
        assert_eq!(p.spent, 3);
        assert!(p.over_budget(3));
    }
}
