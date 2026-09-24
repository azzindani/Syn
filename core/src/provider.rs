//! Provider client (OpenRouter-compatible): endpoint + request-body builder.
//! No network in core: the widget performs transport with a user-supplied key
//! (env `AGENT_API_KEY`, never logged, never stored). Model IDs are defaults
//! overridable per deployment. Mock transport lives in tests.

use crate::router::{Effort, Model, Route};

/// The compiled default for every slot: OpenRouter's own router, which
/// picks a model per request. Neutral on purpose -- no vendor is baked into
/// the binary, and a deployment names its models in `.env`
/// (`AGENT_MODEL_SMALL|STANDARD|CODING|REASONING`), not by code edits.
///
/// Every slot shares it, so an unconfigured install has no failover chain:
/// `fallbacks` skips slots that resolve to the same id.
pub const DEFAULT_MODEL: &str = "openrouter/auto";

/// Default model ID for one slot. Override via deployment config. The match
/// is exhaustive so a new slot has to choose its default here.
pub fn model_id(model: Model) -> &'static str {
    match model {
        Model::Small | Model::Standard | Model::Coding | Model::Reasoning => DEFAULT_MODEL,
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
    /// Roughly how much of the request this message occupies, in
    /// characters. Used by every budget in the loop -- pruning,
    /// summarising, the context guard -- so they all measure the same
    /// thing. Characters, not tokens: `core` has no tokeniser, and a
    /// wrong guess in the safe direction costs nothing.
    pub fn width(&self) -> usize {
        match self {
            Msg::System(c) | Msg::User(c) | Msg::Assistant(c) | Msg::AssistantCalls(c) => c.len(),
            Msg::Tool { id, content } => id.len() + content.len(),
        }
    }

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
/// Why a turn came back with nothing in it.
///
/// "The model ended the turn with no answer and no tool call" was true and
/// useless: four capability runs died that way and the message threw away
/// every fact that would have said which of several very different causes
/// it was. A reasoning model that spent its whole completion budget
/// thinking looks identical, in that sentence, to one that simply refused.
///
/// Hand-scanned like the rest of this module, because core carries no JSON
/// dependency.
pub fn empty_turn_diagnosis(body: &str) -> String {
    let body = body.trim();
    let finish = scan_field(body, "\"finish_reason\"").unwrap_or_else(|| "?".into());
    let reasoning = scan_field(body, "\"reasoning\"").map(|r| r.len()).unwrap_or(0);
    let refusal = scan_field(body, "\"refusal\"").filter(|r| !r.is_empty());
    let mut why = format!("finish_reason={finish}");
    if reasoning > 0 {
        // The tell for a model that thought and then said nothing: the
        // budget went somewhere, just not into an answer.
        why.push_str(&format!(", reasoning={reasoning} chars but no content"));
    }
    if let Some(r) = refusal {
        why.push_str(&format!(", refusal={r:?}"));
    }
    // The body, bounded. Twice now a run has ended on a reply nobody
    // could inspect afterwards, because the diagnosis kept the shape and
    // threw the evidence away.
    let head: String = body.chars().take(240).collect();
    let head = if head.trim().is_empty() { "<empty>".to_string() } else { head };
    if finish == "length" {
        why.push_str(" — the completion was cut off, so raise the budget or lower reasoning effort");
    }
    // When none of the expected fields are there, the body is not the shape
    // this code thinks it is, and no amount of scanning for the right keys
    // will say so. Show what actually arrived.
    if finish == "?" && reasoning == 0 {
        let head: String = body.chars().take(400).collect();
        why.push_str(&format!(", body={head:?}"));
    }
    why.push_str(&format!(", body: {head}"));
    why
}

/// A reply that says it made tool calls and carries none.
///
/// `finish_reason: "tool_calls"` with nothing parseable in `tool_calls` is
/// the provider failing to serialise what the model produced, often
/// alongside a long `reasoning` field showing where the budget went. It is
/// not the model ending its turn, and reading it as one ended a capability
/// run at step 70 of 300 with the workbook half built. Worth waiting and
/// asking again, like any other transport failure.
pub fn announced_calls_but_sent_none(body: &str) -> bool {
    scan_field(body, "\"finish_reason\"").as_deref() == Some("tool_calls")
}

/// The first string value of a field, or None when it is absent or null.
fn scan_field(json: &str, key: &str) -> Option<String> {
    let i = json.find(key)? + key.len();
    let rest = json[i..].trim_start_matches([' ', ':']);
    if !rest.starts_with('"') {
        return None;
    }
    let mut out = String::new();
    let mut it = rest[1..].chars();
    while let Some(c) = it.next() {
        match c {
            '\\' => {
                if let Some(e) = it.next() {
                    out.push(e);
                }
            }
            '"' => return Some(out),
            _ => out.push(c),
        }
    }
    None
}

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
/// Turn a provider error body into one line a human can act on.
///
/// Providers answer failures with a nested JSON envelope; pasting that into a
/// chat window tells the reader nothing and buries the one sentence that
/// matters. The upstream `raw` note is preferred over the generic `message`
/// ("Provider returned error") because it is the one that names the model and
/// says what to do.
pub fn explain_error(status: u16, body: &str) -> String {
    let pick = |key: &str| -> Option<String> {
        let at = body.find(&format!("\"{key}\""))?;
        let rest = body[at..].split_once(':')?.1.trim_start();
        let rest = rest.strip_prefix('"')?;
        let mut out = String::new();
        let mut it = rest.chars();
        while let Some(c) = it.next() {
            match c {
                '\\' => match it.next() {
                    Some('n') => out.push(' '),
                    Some(o) => out.push(o),
                    None => break,
                },
                '"' => return Some(out),
                c => out.push(c),
            }
        }
        None
    };
    let note = pick("raw")
        .or_else(|| pick("message"))
        .unwrap_or_else(|| crate::security::truncate_output(body));
    let note = note.trim();
    match status {
        429 => format!("rate limited (429): {note}"),
        401 | 403 => format!("rejected ({status}): {note}. Check AGENT_API_KEY."),
        _ => format!("provider error ({status}): {note}"),
    }
}

/// Whether a failure is worth retrying on a different model.
///
/// Free-tier slots rate-limit independently, so a 429 on one says nothing
/// about the next. A bad key or a malformed request would fail identically
/// everywhere, and retrying those just burns the other slots.
pub fn worth_another_model(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("429")
        || e.contains("rate limit")
        || e.contains("rate-limit")
        || e.contains("(502)")
        || e.contains("(503)")
        || e.contains("overloaded")
        || e.contains("temporarily unavailable")
        || e.contains("no completion in it")
}

/// Whether waiting and asking the SAME model again is the right move.
///
/// A rate limit is per minute far more often than per day, and an
/// overloaded provider recovers. Walking straight to the next slot on one
/// blip cost two capability runs the model they were meant to be testing:
/// both switched off the model named in the experiment at step ~15 and
/// spent the rest of the run on a weaker one. `08-production-grade.md`
/// lists opencode's exponential backoff as ported; only the model-fallback
/// half of it ever landed.
pub fn worth_waiting(err: &str) -> bool {
    retryable_message(err)
}

// ---- retry, ported from opencode `session/retry.ts` --------------------
//
// See `docs/DIGEST-06-opencode-loop.md` §3. The previous version here was
// a fixed `[2, 6, 15]` with no jitter and, worse, no reading of
// `Retry-After`: the provider states how long to wait and we ignored it.

pub const RETRY_INITIAL_MS: u64 = 2_000;
pub const RETRY_BACKOFF_FACTOR: u64 = 2;
/// Jitter as a percentage of the base delay, so a fleet of clients that
/// were rate-limited together do not all come back in the same instant.
pub const RETRY_JITTER_PCT: u64 = 25;
pub const RETRY_MAX_DELAY_NO_HEADERS_MS: u64 = 30_000;
pub const RETRY_MAX_RETRIES: u32 = 5;

/// `2000 * 2^(attempt-1)`, plus up to 25% jitter, capped at 30s.
///
/// `attempt` is 1-based. `rand_pct` is 0..=99, taken from the caller so
/// this stays a pure function -- `core` has no rng and a test that cannot
/// pin the jitter cannot check the bounds.
pub fn retry_delay_ms(attempt: u32, rand_pct: u64) -> u64 {
    let base = RETRY_INITIAL_MS.saturating_mul(RETRY_BACKOFF_FACTOR.saturating_pow(attempt.saturating_sub(1)));
    let jittered = base.saturating_add(base.saturating_mul(RETRY_JITTER_PCT).saturating_mul(rand_pct % 100) / 10_000);
    jittered.min(RETRY_MAX_DELAY_NO_HEADERS_MS)
}

/// The wait a `Retry-After` header asks for, in milliseconds.
///
/// `retry-after-ms` first, then `retry-after` in seconds. The HTTP-date
/// form of `Retry-After` is not parsed: it needs a clock and a date
/// parser, `core` has neither, and no provider used here sends it. A
/// header we cannot read falls back to the exponential delay rather than
/// being treated as zero.
pub fn retry_after_ms(headers: &str) -> Option<u64> {
    let find = |name: &str| -> Option<&str> {
        headers.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
        })
    };
    if let Some(ms) = find("retry-after-ms")
        && let Ok(n) = ms.parse::<f64>()
        && n.is_finite()
        && n >= 0.0
    {
        return Some(n.ceil() as u64);
    }
    let secs = find("retry-after")?.parse::<f64>().ok()?;
    (secs.is_finite() && secs >= 0.0).then(|| (secs * 1000.0).ceil() as u64)
}

/// Whether this failure is transient, by opencode's seven patterns.
///
/// Ours caught roughly half of these. The ones that matter most in
/// practice and were missing: every network-level failure (`econnreset`,
/// `socket hang up`, `fetch failed`), every timeout phrasing, and
/// "try again later" / "at capacity", which is how several providers
/// phrase a soft limit.
pub fn retryable_message(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    const CODES: &[&str] = &["429", "500", "502", "503", "504", "524"];
    const PHRASES: &[&str] = &[
        // rate limiting
        "rate limit", "rate-limit", "rate_limit", "too many requests", "rate increased too quickly",
        // provider capacity
        "overloaded", "service unavailable", "service_unavailable", "service-unavailable",
        "internal error", "internal_error", "internal server error", "server error", "server_error",
        "provider returned error", "provider_returned_error",
        // network
        "terminated", "fetch failed", "failed to fetch", "network error", "network_error",
        "upstream connect", "connection error", "connection refused", "connection lost",
        "socket connection was closed", "socket hang up", "reset before headers",
        "getaddrinfo", "enotfound", "eai_again", "econnrefused", "econnreset", "etimedout",
        // timeouts
        "timeout", "timed out", "time out",
        // explicit invitations to retry
        "try your request again", "retry your request", "resource exhausted", "resource_exhausted",
        "try again later", "try again in", "currently at capacity", "temporarily at capacity",
        // the two this project found for itself
        "no completion in it", "announced tool calls but sent none",
    ];
    // A context overflow is never retried: asking the same question with
    // the same oversized transcript gets the same answer, slower.
    if m.contains("413") || m.contains("too large") || m.contains("context length") {
        return false;
    }
    CODES.iter().any(|c| m.contains(c)) || PHRASES.iter().any(|p| m.contains(p))
}

/// A 200 that carries no completion at all.
///
/// The second member of the family `error_in_ok_body` opened. A capability
/// run died at step 11 on an HTTP 200 whose body was zero bytes long: no
/// choices, no error object, nothing. The loop had no way to tell that from
/// a model with nothing to say, so it reported the model as having ended
/// the turn -- the same wrong verdict, arrived at a different way.
///
/// A completion without a `choices` key is not a completion, whatever the
/// status line says. Saying so here puts it on the retry path instead.
pub fn no_completion_in_ok_body(body: &str) -> Option<String> {
    if body.contains(r#""choices""#) {
        return None;
    }
    // The body, not just its length. Reporting "220 bytes" and throwing
    // the content away is the same mistake `empty_turn_diagnosis` exists
    // to undo: two capability runs switched model on this reply and
    // nobody could say what it was.
    let head: String = body.chars().take(200).collect();
    Some(format!(
        "the provider returned a 200 with no completion in it ({} bytes): not a model turn. body: {}",
        body.len(),
        if head.trim().is_empty() { "<empty>".into() } else { head }
    ))
}

/// An upstream failure reported inside a 200.
///
/// OpenRouter answers a provider outage with HTTP 200 and an `error` object
/// where the choices should be:
///
/// ```text
/// {"id":"gen-...","error":{"message":"Upstream error from Nvidia: Service
///  temporarily overloaded","code":503,"metadata":{...}}}
/// ```
///
/// Checking the status code alone let that body through to the agent, which
/// found no content and no tool calls in it and reported that the model had
/// ended the turn with nothing to say. Four capability runs were scored on
/// that reading. The models had not given up; they were never asked.
pub fn error_in_ok_body(body: &str) -> Option<String> {
    // An `error` key before any `choices` key: a normal completion can
    // mention "error" inside a message, so position is what distinguishes
    // an error envelope from a reply that talks about one.
    let at = body.find(r#""error""#)?;
    if let Some(ch) = body.find(r#""choices""#)
        && ch < at
    {
        return None;
    }
    let msg = scan_field(&body[at..], r#""message""#).unwrap_or_else(|| "no message".into());
    let code = scan_field(&body[at..], r#""error_type""#)
        .or_else(|| digits_after(&body[at..], r#""code""#))
        .unwrap_or_else(|| "unknown".into());
    Some(format!("upstream failed inside a 200 ({code}): {msg}"))
}

fn digits_after(json: &str, key: &str) -> Option<String> {
    let i = json.find(key)? + key.len();
    let rest = json[i..].trim_start_matches([' ', ':']);
    let n: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (!n.is_empty()).then_some(n)
}

/// Returns (http_status, response_body).
pub fn send_via_curl(base_url: &str, api_key_env: &str, body: &str) -> Result<(u16, String), String> {
    // The stored credential for this endpoint first, then the env var.
    // `auth.json` is keyed by provider, so adding a second provider is a
    // row in a file rather than a new env var invented for it -- and an
    // OAuth token that has expired is skipped rather than sent, because a
    // stale token answers 401 and reads exactly like a bad key.
    let key = crate::auth::resolve(base_url, api_key_env)
        .ok_or_else(|| format!("no credential for {base_url}: set {api_key_env} or add it to .agent/auth.json"))?;
    let url = format!("{base_url}/chat/completions");
    let out = std::process::Command::new("curl")
        // Five minutes: a reply that is not streamed arrives all at once, and a
        // model that thinks first used to be cut off at sixty seconds and
        // reported as "no HTTP response".
        .args(["-sS", "-m", "300", "-X", "POST", &url])
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

/// How long a streamed reply may go quiet before it is given up on.
/// Silence, not length: a model reasoning for ten minutes is working, and
/// the providers that make it wait send keep-alive comments meanwhile. A
/// connection with nothing at all on it for this long is dead.
pub fn stream_idle_secs() -> u64 {
    std::env::var("AGENT_STREAM_IDLE_SECS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(180).clamp(10, 3600)
}

/// Whether model replies are streamed. On unless `AGENT_STREAM=0`, for a
/// gateway that mishandles `"stream":true`.
pub fn streaming_on() -> bool {
    !matches!(std::env::var("AGENT_STREAM").ok().as_deref().map(str::trim), Some("0" | "false" | "off" | "no"))
}

/// The most a streamed reply may hold. Well past any real completion; a
/// connection that keeps sending past it is refused, not truncated.
const MAX_REPLY: usize = 32 << 20;

/// Like [`send_via_curl`], but the reply is streamed: `on` sees the text and
/// the reasoning as they arrive (batched a few times a second), and the
/// result is folded back into the shape a non-streamed reply has, so every
/// check downstream reads it unchanged. The only time limit is on silence
/// ([`stream_idle_secs`]) plus a generous ceiling for the whole reply.
pub fn send_streaming(base_url: &str, api_key_env: &str, body: &str, on: &mut dyn FnMut(crate::sse::Kind, &str)) -> Result<(u16, String), String> {
    use std::io::{BufRead, Write};
    let key = crate::auth::resolve(base_url, api_key_env)
        .ok_or_else(|| format!("no credential for {base_url}: set {api_key_env} or add it to .agent/auth.json"))?;
    let url = format!("{base_url}/chat/completions");
    let body = crate::sse::with_stream_flag(body);
    let idle = stream_idle_secs().to_string();
    let mut child = std::process::Command::new("curl")
        // -N: hand each chunk over as it arrives instead of filling a buffer
        // first. --speed-limit/--speed-time: fewer than 1 byte a second for
        // `idle` seconds is a dead connection.
        .args(["-sS", "-N", "--max-time", "1800", "--speed-limit", "1", "--speed-time", &idle, "-X", "POST", &url])
        .args(["-H", "Content-Type: application/json"])
        .args(["-H", "Accept: text/event-stream"])
        .args(["-H", &format!("Authorization: Bearer {key}")])
        .args(["--data-binary", "@-"])
        .arg("-w")
        .arg("\n%{http_code}")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl spawn failed: {e}"))?;
    // curl reads all of `@-` before it sends anything, so the body is
    // written and closed before the first byte of the reply is read. A
    // BrokenPipe means curl died early; its reason is on stderr.
    if let Some(mut sink) = child.stdin.take()
        && let Err(e) = sink.write_all(body.as_bytes())
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("stdin: {e}"));
    }
    let mut fold = crate::sse::Fold::new();
    let mut code = String::new();
    let mut total = 0usize;
    let mut oversized = false;
    if let Some(out) = child.stdout.take() {
        let mut out = std::io::BufReader::new(out);
        let mut th = crate::sse::Throttle::new(on);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match out.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => total += n,
            }
            if total > MAX_REPLY {
                oversized = true;
                let _ = child.kill();
                break;
            }
            // `-w` puts the status after a newline and ends without one,
            // so the one unterminated line is the status, never the body.
            if buf.last() != Some(&b'\n') {
                code = String::from_utf8_lossy(&buf).into_owned();
                break;
            }
            fold.line(&String::from_utf8_lossy(&buf), &mut |k, t| th.push(k, t));
            th.tick();
        }
        th.flush();
    }
    let res = child.wait_with_output().map_err(|e| format!("curl wait: {e}"))?;
    if oversized {
        return Err(format!("the reply passed {} MB and was refused", MAX_REPLY >> 20));
    }
    let stderr = String::from_utf8_lossy(&res.stderr).trim().to_string();
    let code: u16 = code.trim().parse().unwrap_or(0);
    // As in `send_via_curl`: 000 is no HTTP response at all, and the reason
    // only exists on stderr. A stream that went quiet past the idle limit
    // lands here too, as curl's "Operation too slow".
    if code == 0 {
        return Err(if stderr.is_empty() {
            format!("curl produced no HTTP response ({})", res.status)
        } else if stderr.contains("too slow") {
            format!("the model sent nothing for {idle}s and was given up on (AGENT_STREAM_IDLE_SECS): {stderr}")
        } else {
            format!("curl transport failed: {stderr}")
        });
    }
    Ok((code, fold.finish()))
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
        assert!(body.contains(r#""model":"openrouter/auto""#));
        assert!(body.contains("\\\"hi\\\""));
        assert!(body.contains("\\n"));
        let m = Mock { seen_url: Default::default(), seen_body: Default::default() };
        m.post(DEFAULT_BASE_URL, "AGENT_API_KEY", &body).unwrap();
        assert!(m.seen_url.borrow().contains("openrouter.ai"));
        assert!(m.seen_body.borrow().contains(DEFAULT_MODEL));
    }

    #[test]
    fn every_slot_defaults_to_the_neutral_router() {
        for m in crate::router::Model::ALL {
            assert_eq!(model_id(m), DEFAULT_MODEL, "{m:?}");
        }
        let r = route(TaskKind::VisionFallback);
        assert!(request_body(r, "", "").contains(DEFAULT_MODEL));
    }

    #[test]
    fn stream_body_flags_sse() {
        let b = stream_body(route(TaskKind::Routine), "s", "u");
        assert!(b.contains(r#""stream":true"#));
        assert!(b.contains(DEFAULT_MODEL));
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
        assert_eq!(p.pick(TaskKind::Skim).model, Model::Small);
        p.override_model = Some(Model::Coding);
        let r = p.pick(TaskKind::Skim);
        assert_eq!(r.model, Model::Coding);
        p.note_spend(r);
        assert!(!p.over_budget(10));
        assert!(p.over_budget(1));
    }

    /// The exact body OpenRouter returned when the free Standard slot was
    /// exhausted. Pasting this into a chat window is what the fix is for.
    const RATE_LIMIT_BODY: &str = r#"{"error":{"message":"Provider returned error","code":429,"metadata":{"raw":"qwen/qwen3.8-27b:free is temporarily rate-limited upstream. Please retry shortly, or add your own key to accumulate your rate limits: https://openrouter.ai/settings/integrations","provider_name":"ModelRun","is_byok":false}},"user_id":"user_3ByZ"}"#;

    #[test]
    fn a_rate_limit_body_becomes_one_readable_line() {
        let m = explain_error(429, RATE_LIMIT_BODY);
        assert!(m.starts_with("rate limited (429):"), "{m}");
        // The upstream note names the model; the generic wrapper does not.
        assert!(m.contains("qwen/qwen3.8-27b:free"), "{m}");
        assert!(!m.contains("Provider returned error"), "the generic message is not the useful one: {m}");
        assert!(!m.contains("user_id"), "no envelope noise: {m}");
        assert!(m.lines().count() == 1, "must stay one line: {m}");
    }

    #[test]
    fn a_bad_key_says_which_setting_to_check() {
        let m = explain_error(401, r#"{"error":{"message":"No auth credentials found"}}"#);
        assert!(m.contains("No auth credentials found"), "{m}");
        assert!(m.contains("AGENT_API_KEY"), "{m}");
    }

    #[test]
    fn an_unparseable_body_still_produces_something() {
        let m = explain_error(500, "<html>gateway error</html>");
        assert!(m.contains("500"));
        assert!(m.contains("gateway"), "{m}");
    }

    #[test]
    fn only_transient_failures_are_worth_another_model() {
        assert!(worth_another_model(&explain_error(429, RATE_LIMIT_BODY)));
        assert!(worth_another_model("provider error (503): upstream unavailable"));
        // A bad key or a malformed request fails the same way on every slot,
        // so retrying would burn the others and report four identical errors.
        assert!(!worth_another_model(&explain_error(401, "{}")));
        assert!(!worth_another_model("provider error (400): bad request"));
        assert!(!worth_another_model("curl transport failed: could not resolve host"));
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

    #[test]
    fn an_empty_turn_says_which_kind_of_empty_it_was() {
        // A reasoning model that burned its budget thinking and a model
        // that refused both arrive as "no answer and no tool call". The
        // whole point of the diagnosis is that they are not the same
        // problem and do not have the same fix.
        let cut = r#"{"choices":[{"finish_reason":"length","message":{"content":null,"reasoning":"thinking hard about the workbook"}}]}"#;
        let d = empty_turn_diagnosis(cut);
        assert!(d.contains("finish_reason=length"), "{d}");
        assert!(d.contains("reasoning="), "{d}");
        assert!(d.contains("cut off"), "{d}");

        let refused = r#"{"choices":[{"finish_reason":"stop","message":{"content":null,"refusal":"I cannot help with that"}}]}"#;
        let d2 = empty_turn_diagnosis(refused);
        assert!(d2.contains("finish_reason=stop"), "{d2}");
        assert!(d2.contains("refusal"), "{d2}");
        assert!(!d2.contains("cut off"), "a clean stop is not a truncation: {d2}");

        // Nothing to report is still a sentence, not a panic.
        let bare = r#"{"choices":[{"finish_reason":"stop","message":{"content":null}}]}"#;
        assert!(empty_turn_diagnosis(bare).contains("finish_reason=stop"));
    }

    #[test]
    fn an_upstream_failure_inside_a_200_is_an_error_not_an_empty_turn() {
        // The body that cost four capability runs their result. It arrives
        // with HTTP 200, so the status check passed it straight through to
        // the agent, which read "no content, no tool calls" as the model
        // giving up.
        let overloaded = r#"{"id":"gen-1789834235-vZqrie8","error":{"message":"Upstream error from Nvidia: Service temporarily overloaded","code":503,"metadata":{"error_type":"provider_overloaded"}}}"#;
        let got = error_in_ok_body(overloaded).expect("an error envelope is an error");
        assert!(got.contains("provider_overloaded"), "{got}");
        assert!(got.contains("temporarily overloaded"), "{got}");
        // And it has to reach the fallback chain, or the run dies on one
        // provider having a bad minute.
        assert!(worth_another_model(&got), "an overloaded provider is worth another model: {got}");

        // A real completion is not an error, even when it says the word.
        let fine = r#"{"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"the write returned an error, so I tried again"}}]}"#;
        assert_eq!(error_in_ok_body(fine), None, "a reply that mentions an error is still a reply");
    }

    #[test]
    fn a_200_with_nothing_in_it_is_a_provider_failure_not_a_silent_model() {
        // Arm B of the manual experiment died here at step 11, eleven
        // correct operations in, and it was scored as the model quitting.
        // It is the same mistake as reading a 503 inside a 200 as silence.
        let nothing = "";
        let why = no_completion_in_ok_body(nothing).expect("an empty body is not a turn");
        assert!(why.contains("0 bytes"), "{why}");
        assert!(worth_another_model(&why), "it has to reach the fallback chain: {why}");

        // Truncated JSON is the same case: no choices, no completion.
        assert!(no_completion_in_ok_body(r#"{"id":"gen-1"}"#).is_some());

        // A real completion passes through untouched, including an empty
        // one -- a model that genuinely said nothing still sent choices,
        // and that IS a model turn, to be nudged rather than retried.
        let real = r#"{"choices":[{"finish_reason":"stop","message":{"content":""}}]}"#;
        assert_eq!(no_completion_in_ok_body(real), None);
    }

    #[test]
    fn a_transient_failure_is_worth_waiting_for_before_changing_model() {
        // Both arms of experiment 3 abandoned the model under test at
        // step ~15 on a single blip, and ran the rest on a weaker slot.
        for e in [
            "rate limited (429): temporarily rate-limited upstream",
            "upstream failed inside a 200 (provider_overloaded): Service temporarily overloaded",
            "the provider returned a 200 with no completion in it (220 bytes)",
        ] {
            assert!(worth_waiting(e), "should wait and re-ask: {e}");
        }
        // A real refusal or a bad key is not transient: waiting just
        // spends the clock to be told the same thing.
        assert!(!worth_waiting("provider error (401): invalid api key"));
        assert!(!worth_waiting("provider error (413): Request too large"));
    }

    #[test]
    fn a_non_completion_reports_what_it_actually_was() {
        let why = no_completion_in_ok_body(r#"{"id":"gen-1","detail":"quota"}"#).unwrap();
        assert!(why.contains("quota"), "the body has to survive the diagnosis: {why}");
        assert!(no_completion_in_ok_body("").unwrap().contains("<empty>"));
    }

    #[test]
    fn announced_calls_with_none_attached_is_a_transport_failure() {
        // Ended a run at step 70 of 300: finish_reason said tool_calls,
        // 876 characters of reasoning arrived, and the calls themselves
        // did not. The loop read that as the model having nothing to say.
        let broken = r#"{"choices":[{"finish_reason":"tool_calls","message":{"content":null,"reasoning":"working out the next write"}}]}"#;
        assert!(announced_calls_but_sent_none(broken));
        assert!(worth_waiting("the model announced tool calls but sent none"));

        // A model that genuinely stopped is not this, and must still be
        // believed rather than walked round the loop.
        let done = r#"{"choices":[{"finish_reason":"stop","message":{"content":null}}]}"#;
        assert!(!announced_calls_but_sent_none(done));

        // And the diagnosis now carries the evidence with it.
        assert!(empty_turn_diagnosis(broken).contains("body: "));
    }

    #[test]
    fn retry_delay_follows_opencodes_curve() {
        // 2000 * 2^(n-1), plus up to 25% jitter, capped at 30s.
        assert_eq!(retry_delay_ms(1, 0), 2_000);
        assert_eq!(retry_delay_ms(2, 0), 4_000);
        assert_eq!(retry_delay_ms(3, 0), 8_000);
        // Jitter only ever adds, and never more than a quarter.
        for pct in [0u64, 1, 50, 99] {
            let d = retry_delay_ms(3, pct);
            assert!((8_000..=10_000).contains(&d), "attempt 3 with jitter {pct} gave {d}");
        }
        // The cap holds however far the ladder is climbed.
        assert_eq!(retry_delay_ms(20, 99), RETRY_MAX_DELAY_NO_HEADERS_MS);
    }

    #[test]
    fn a_retry_after_header_beats_the_ladder() {
        // The provider states the wait and the old code ignored it.
        assert_eq!(retry_after_ms("retry-after-ms: 1500"), Some(1_500));
        assert_eq!(retry_after_ms("Retry-After: 12"), Some(12_000));
        assert_eq!(retry_after_ms("retry-after: 0.5"), Some(500));
        // A header we cannot read is not a zero wait.
        assert_eq!(retry_after_ms("retry-after: Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(retry_after_ms("content-type: application/json"), None);
    }

    #[test]
    fn the_retryable_patterns_cover_what_ours_missed() {
        // Every one of these is from opencode's list and none of them
        // matched the predicate this replaced.
        for e in [
            "socket hang up",
            "ECONNRESET",
            "fetch failed",
            "request timed out",
            "resource exhausted",
            "please try again later",
            "the service is currently at capacity",
            "upstream connect error",
            "provider returned error",
        ] {
            assert!(retryable_message(e), "should be retryable: {e}");
        }
        // And the ones that must never be retried: the same oversized
        // transcript gets the same answer, slower.
        for e in [
            "provider error (413): Request too large",
            "context length exceeded",
            "provider error (401): invalid api key",
        ] {
            assert!(!retryable_message(e), "must not be retried: {e}");
        }
    }
}
