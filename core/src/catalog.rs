//! The model catalog: what each provider says it serves, today.
//!
//! The picker used to offer the four `.env` slots by name, which meant a
//! human choosing between "Standard" and "Coding" with no idea what either
//! was, and a new model reaching the console only when someone edited a
//! file. Every OpenAI-compatible host answers `GET {base}/models`, so the
//! list is asked for instead of written down: OpenRouter's is public and
//! rich (context, prices, whether it can reason, whether it can call
//! tools), OpenCode Zen's and a local server's are the bare OpenAI shape.
//! One parser takes both and keeps what is there.
//!
//! The answer is cached beside the chats (`models.json`), so a console
//! started offline still has the last list, and the console refreshes it
//! in the background (`ui`), so it does not go stale while open.
//!
//! What a provider sends is untrusted like any other network input: names
//! and ids are capped, a catalog is capped, and nothing from it is ever
//! run. An id only becomes the `model` field of a request the human sends.

use crate::json::{self, Value};
use std::path::PathBuf;

/// OpenCode Zen: the pay-as-you-go gateway OpenCode itself uses. Named here
/// because the human asked for it by name; any other host is one
/// `AGENT_BASE_URL` away and gets the same treatment.
pub const OPENCODE_BASE_URL: &str = "https://opencode.ai/zen/v1";
pub const OPENCODE_KEY_ENV: &str = "AGENT_API_KEY_OPENCODE";
pub const OPENROUTER_KEY_ENV: &str = "AGENT_API_KEY_OPENROUTER";

/// Refetch when the cached list is older than this. Providers add models
/// weekly and retire free ones without notice; a quarter of an hour is
/// fresh enough to never offer a model that vanished yesterday, and slow
/// enough that a console left open all day asks a few dozen times.
pub const FRESH_SECS: u64 = 15 * 60;

/// OpenRouter lists ~350. A thousand leaves room to grow and still stops a
/// broken or hostile endpoint from filling the page.
const MAX_MODELS: usize = 1000;
const MAX_ID: usize = 200;
const MAX_NAME: usize = 120;

/// One place models can be sent to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    /// Short and stable, used on the wire: `model <provider> <id>`.
    pub name: String,
    /// What the picker shows.
    pub label: String,
    pub base_url: String,
    /// The NAME of the env var holding its key, never the key.
    pub key_env: String,
}

/// One model, as much as the provider told us.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: String,
    pub name: String,
    /// Tokens, 0 when not said.
    pub context: u64,
    /// Dollars per million tokens in and out, when the provider says.
    pub price_in: Option<f64>,
    pub price_out: Option<f64>,
    /// Whether it accepts a reasoning effort. None: the provider did not
    /// say, which is every host but OpenRouter.
    pub thinks: Option<bool>,
    pub vision: Option<bool>,
}

impl Entry {
    fn bare(id: &str) -> Self {
        Entry { id: id.to_string(), name: id.to_string(), context: 0, price_in: None, price_out: None, thinks: None, vision: None }
    }
}

/// Which providers this deployment can reach, in picker order.
///
/// The configured endpoint first, under its own name when it is one we
/// know. Then OpenRouter and OpenCode when a key for them exists, so a
/// human with both keys picks from both without editing the base URL. A
/// provider with no key at all is still listed when it is the configured
/// one: OpenRouter's catalog is public, and seeing what is on offer is how
/// someone decides to get a key.
pub fn providers_with(lookup: impl Fn(&str) -> Option<String>) -> Vec<Provider> {
    let nonblank = |k: &str| lookup(k).is_some_and(|v| !v.trim().is_empty());
    let base = crate::config::base_url_with(&lookup);
    let mut out = vec![Provider {
        name: name_for(&base),
        label: label_for(&base),
        base_url: base.clone(),
        key_env: crate::config::API_KEY_ENV.to_string(),
    }];
    for (url, key) in [(crate::provider::DEFAULT_BASE_URL, OPENROUTER_KEY_ENV), (OPENCODE_BASE_URL, OPENCODE_KEY_ENV)] {
        if url != base && nonblank(key) {
            out.push(Provider { name: name_for(url), label: label_for(url), base_url: url.to_string(), key_env: key.to_string() });
        }
    }
    out
}

pub fn providers() -> Vec<Provider> {
    providers_with(|k| std::env::var(k).ok())
}

/// The provider called `name`, from those this deployment has.
pub fn provider_named(name: &str) -> Option<Provider> {
    providers().into_iter().find(|p| p.name == name)
}

fn name_for(base: &str) -> String {
    if base == crate::provider::DEFAULT_BASE_URL {
        return "openrouter".into();
    }
    if base == OPENCODE_BASE_URL {
        return "opencode".into();
    }
    // Any other host is named after itself, so two custom endpoints cannot
    // share a name. `http://127.0.0.1:11434/v1` -> `127.0.0.1:11434`.
    let host = base.split("://").nth(1).unwrap_or(base).split('/').next().unwrap_or(base);
    host.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '-')).take(60).collect()
}

fn label_for(base: &str) -> String {
    match name_for(base).as_str() {
        "openrouter" => "OpenRouter".into(),
        "opencode" => "OpenCode Zen".into(),
        other => other.to_string(),
    }
}

/// Read a `/models` reply. OpenRouter's rich shape and the bare OpenAI
/// one are the same list with more or fewer fields; take what is there.
///
/// Dropped: models that cannot call tools (Syn is nothing but tool calls,
/// so offering one is offering a model that will answer "I can't do that"),
/// and models whose output is not text (image generators). Only dropped
/// when the provider SAYS so: a bare list says nothing and keeps all.
pub fn parse(body: &str) -> Result<Vec<Entry>, String> {
    let v = json::parse(body.trim()).map_err(|e| format!("not JSON: {e}"))?;
    let list = v
        .get("data")
        .and_then(Value::as_arr)
        .or(v.as_arr())
        .ok_or("no `data` list in the reply")?;
    let mut out = Vec::new();
    for m in list {
        let Some(id) = m.get("id").and_then(Value::as_str).map(str::trim) else { continue };
        if id.is_empty() || id.len() > MAX_ID || id.chars().any(|c| c.is_whitespace() || c.is_control()) {
            continue;
        }
        let params: Option<Vec<&str>> = m
            .get("supported_parameters")
            .and_then(Value::as_arr)
            .map(|a| a.iter().filter_map(Value::as_str).collect());
        if params.as_ref().is_some_and(|p| !p.contains(&"tools")) {
            continue;
        }
        let modes = |key: &str| -> Option<Vec<&str>> {
            m.at(&["architecture", key]).and_then(Value::as_arr).map(|a| a.iter().filter_map(Value::as_str).collect())
        };
        if modes("output_modalities").is_some_and(|o| !o.contains(&"text")) {
            continue;
        }
        let mut e = Entry::bare(id);
        if let Some(n) = m.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()) {
            e.name = n.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
        }
        e.context = m
            .get("context_length")
            .or_else(|| m.at(&["top_provider", "context_length"]))
            .and_then(num)
            .map(|n| n.max(0.0) as u64)
            .unwrap_or(0);
        // Per token, as text: "0.000003". Negative means "varies" (a
        // router), which is not a price to show.
        // Rounded here, once, so the cache reads back exactly what it wrote.
        let per_million =
            |k: &str| m.at(&["pricing", k]).and_then(num).filter(|p| *p >= 0.0).map(|p| (p * 1e10).round() / 1e4);
        e.price_in = per_million("prompt");
        e.price_out = per_million("completion");
        e.thinks = params.as_ref().map(|p| p.contains(&"reasoning") || p.contains(&"reasoning_effort"));
        e.vision = modes("input_modalities").map(|i| i.contains(&"image"));
        out.push(e);
        if out.len() == MAX_MODELS {
            break;
        }
    }
    Ok(out)
}

/// A JSON number, or a number written as a string (OpenRouter's prices).
fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Num(n) | Value::Str(n) => n.trim().parse().ok(),
        _ => None,
    }
}

/// Ask one provider for its list. The key is sent when there is one:
/// OpenRouter answers without, some hosts do not.
pub fn fetch(p: &Provider) -> Result<Vec<Entry>, String> {
    let url = format!("{}/models", p.base_url);
    let mut cmd = std::process::Command::new("curl");
    cmd.args(["-sS", "-m", "30", "--compressed", &url, "-w", "\n%{http_code}"]);
    if let Some(key) = crate::auth::resolve(&p.base_url, &p.key_env) {
        // On stdin via -H @-, not argv: argv is readable by every process
        // on the machine for as long as curl runs.
        cmd.args(["-H", "@-"]);
        return run(cmd, Some(format!("Authorization: Bearer {key}\n")));
    }
    run(cmd, None)
}

fn run(mut cmd: std::process::Command, stdin: Option<String>) -> Result<Vec<Entry>, String> {
    use std::io::Write;
    use std::process::Stdio;
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("curl spawn failed: {e}"))?;
    if let (Some(text), Some(mut sink)) = (stdin, child.stdin.take()) {
        let _ = sink.write_all(text.as_bytes());
    }
    let res = child.wait_with_output().map_err(|e| format!("curl wait: {e}"))?;
    let text = String::from_utf8_lossy(&res.stdout);
    let (payload, code) = text.rsplit_once('\n').unwrap_or((&text, "0"));
    match code.trim().parse::<u16>().unwrap_or(0) {
        0 => Err(format!("no answer: {}", String::from_utf8_lossy(&res.stderr).trim())),
        200 => parse(payload),
        c => Err(format!("HTTP {c}")),
    }
}

/// One provider's list as last fetched.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub provider: Provider,
    /// Seconds since the epoch of the last successful fetch, 0 for never.
    pub fetched: u64,
    /// Why the last attempt failed, when it did. The entries from the
    /// attempt before are kept: an offline minute should not empty the
    /// picker.
    pub error: Option<String>,
    pub entries: Vec<Entry>,
}

/// Fetch every provider that is stale, keeping the last good list for any
/// that fails. `fetch` is passed in so the policy is testable offline.
pub fn refresh(
    providers: &[Provider],
    previous: &[Snapshot],
    now: u64,
    force: bool,
    fetch: impl Fn(&Provider) -> Result<Vec<Entry>, String>,
) -> Vec<Snapshot> {
    providers
        .iter()
        .map(|p| {
            let old = previous.iter().find(|s| s.provider.base_url == p.base_url);
            if let Some(o) = old
                && !force
                && o.error.is_none()
                && now.saturating_sub(o.fetched) < FRESH_SECS
            {
                return Snapshot { provider: p.clone(), ..o.clone() };
            }
            match fetch(p) {
                Ok(entries) => Snapshot { provider: p.clone(), fetched: now, error: None, entries },
                Err(e) => Snapshot {
                    provider: p.clone(),
                    fetched: old.map(|o| o.fetched).unwrap_or(0),
                    error: Some(e.chars().take(300).collect()),
                    entries: old.map(|o| o.entries.clone()).unwrap_or_default(),
                },
            }
        })
        .collect()
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Where the list is kept between runs.
pub fn cache_path() -> PathBuf {
    crate::chats::home().join("models.json")
}

/// The cache, and exactly what the console's `/models` serves.
pub fn to_json(snaps: &[Snapshot]) -> String {
    let opt = |p: Option<f64>| p.filter(|x| x.is_finite()).map(|x| Value::Num(format!("{x}"))).unwrap_or(Value::Null);
    let flag = |b: Option<bool>| b.map(Value::Bool).unwrap_or(Value::Null);
    let providers = snaps
        .iter()
        .map(|s| {
            json::obj(vec![
                ("name", json::s(&s.provider.name)),
                ("label", json::s(&s.provider.label)),
                ("base", json::s(&s.provider.base_url)),
                // Whether a key is on hand, never the key: the picker says
                // "add a key" rather than letting a turn fail on a 401.
                ("ready", Value::Bool(crate::auth::resolve(&s.provider.base_url, &s.provider.key_env).is_some())),
                ("fetched", Value::Num(s.fetched.to_string())),
                ("error", s.error.as_deref().map(json::s).unwrap_or(Value::Null)),
                (
                    "models",
                    Value::Arr(
                        s.entries
                            .iter()
                            .map(|e| {
                                json::obj(vec![
                                    ("id", json::s(&e.id)),
                                    ("name", json::s(&e.name)),
                                    ("context", Value::Num(e.context.to_string())),
                                    ("in", opt(e.price_in)),
                                    ("out", opt(e.price_out)),
                                    ("thinks", flag(e.thinks)),
                                    ("vision", flag(e.vision)),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ])
        })
        .collect();
    json::obj(vec![("providers", Value::Arr(providers))]).to_json()
}

/// Read the cache back. The providers come from what is configured now,
/// not from the file: a key removed from `.env` removes its provider.
pub fn from_json(text: &str, providers: &[Provider]) -> Vec<Snapshot> {
    let Ok(v) = json::parse(text) else { return Vec::new() };
    let Some(list) = v.get("providers").and_then(Value::as_arr) else { return Vec::new() };
    let mut out = Vec::new();
    for p in providers {
        let Some(row) = list.iter().find(|r| r.get("base").and_then(Value::as_str) == Some(&p.base_url)) else { continue };
        let entries = row
            .get("models")
            .and_then(Value::as_arr)
            .unwrap_or(&[])
            .iter()
            .filter_map(|m| {
                let id = m.get("id").and_then(Value::as_str)?;
                let mut e = Entry::bare(id);
                e.name = m.get("name").and_then(Value::as_str).unwrap_or(id).to_string();
                e.context = m.get("context").and_then(num).map(|n| n as u64).unwrap_or(0);
                e.price_in = m.get("in").and_then(num);
                e.price_out = m.get("out").and_then(num);
                e.thinks = m.get("thinks").and_then(bool_of);
                e.vision = m.get("vision").and_then(bool_of);
                Some(e)
            })
            .collect();
        out.push(Snapshot {
            provider: p.clone(),
            fetched: row.get("fetched").and_then(num).map(|n| n as u64).unwrap_or(0),
            error: row.get("error").and_then(Value::as_str).map(str::to_string),
            entries,
        });
    }
    out
}

fn bool_of(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

pub fn load() -> Vec<Snapshot> {
    std::fs::read_to_string(cache_path()).map(|t| from_json(&t, &providers())).unwrap_or_default()
}

pub fn save(snaps: &[Snapshot]) {
    let p = cache_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Written aside and renamed, so a console reading it mid-write never
    // sees half a list.
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, to_json(snaps)).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// Load, refresh what is stale, save. What the CLI's `models` and the
/// console's background thread both call.
pub fn update(force: bool) -> Vec<Snapshot> {
    let snaps = refresh(&providers(), &load(), now(), force, fetch);
    save(&snaps);
    snaps
}

#[cfg(test)]
mod tests {
    use super::*;

    // The shape OpenRouter's /api/v1/models documents, trimmed to the
    // fields read here, plus one of each thing that must be dropped.
    const OPENROUTER: &str = r#"{"data":[
      {"id":"vendor/thinker","name":"Vendor: Thinker","context_length":200000,
       "architecture":{"input_modalities":["text","image"],"output_modalities":["text"]},
       "pricing":{"prompt":"0.000003","completion":"0.000015"},
       "supported_parameters":["tools","tool_choice","reasoning","include_reasoning","max_tokens"]},
      {"id":"vendor/quick:free","name":"Vendor: Quick (free)","context_length":32768,
       "architecture":{"input_modalities":["text"],"output_modalities":["text"]},
       "pricing":{"prompt":"0","completion":"0"},
       "supported_parameters":["tools","temperature"]},
      {"id":"vendor/chatty","name":"No tools","supported_parameters":["temperature"]},
      {"id":"vendor/painter","name":"Images out","architecture":{"output_modalities":["image"]},"supported_parameters":["tools"]},
      {"id":"openrouter/auto","name":"Auto Router","context_length":2000000,
       "pricing":{"prompt":"-1","completion":"-1"},"supported_parameters":["tools","reasoning"]},
      {"id":"has space","name":"bad id"},
      {"name":"no id at all"}
    ]}"#;

    // The bare OpenAI list, which is what OpenCode Zen and local servers send.
    const BARE: &str = r#"{"object":"list","data":[{"id":"big-pickle","object":"model","created":1,"owned_by":"opencode"},{"id":"kimi-k2","object":"model"}]}"#;

    #[test]
    fn a_rich_catalog_keeps_prices_context_and_whether_it_thinks() {
        let got = parse(OPENROUTER).unwrap();
        let ids: Vec<&str> = got.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["vendor/thinker", "vendor/quick:free", "openrouter/auto"]);
        let t = &got[0];
        assert_eq!(t.name, "Vendor: Thinker");
        assert_eq!(t.context, 200_000);
        assert!((t.price_in.unwrap() - 3.0).abs() < 1e-9 && (t.price_out.unwrap() - 15.0).abs() < 1e-9);
        assert_eq!((t.thinks, t.vision), (Some(true), Some(true)));
        assert_eq!(got[1].thinks, Some(false));
        assert_eq!(got[1].price_in, Some(0.0));
        // A router's "-1" means it varies, which is not a price.
        assert_eq!(got[2].price_in, None);
    }

    #[test]
    fn a_model_that_cannot_call_tools_is_not_offered() {
        // Syn is nothing but tool calls.
        let got = parse(OPENROUTER).unwrap();
        assert!(got.iter().all(|e| e.id != "vendor/chatty" && e.id != "vendor/painter"));
    }

    #[test]
    fn a_bare_list_is_kept_whole_because_it_says_nothing_to_drop_on() {
        let got = parse(BARE).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], Entry::bare("big-pickle"));
        assert_eq!(got[0].thinks, None, "unknown is not no");
    }

    #[test]
    fn a_reply_that_is_not_a_catalog_says_so() {
        assert!(parse("<html>").unwrap_err().contains("not JSON"));
        assert!(parse(r#"{"error":{"message":"no"}}"#).unwrap_err().contains("data"));
    }

    #[test]
    fn a_huge_catalog_is_capped_rather_than_trusted() {
        let many: Vec<String> = (0..MAX_MODELS + 50).map(|i| format!(r#"{{"id":"m{i}"}}"#)).collect();
        assert_eq!(parse(&format!(r#"{{"data":[{}]}}"#, many.join(","))).unwrap().len(), MAX_MODELS);
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn openrouter_is_the_provider_when_nothing_is_configured() {
        let got = providers_with(env(&[]));
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].name.as_str(), got[0].key_env.as_str()), ("openrouter", "AGENT_API_KEY"));
    }

    #[test]
    fn a_second_provider_appears_when_its_key_does() {
        let got = providers_with(env(&[("AGENT_API_KEY_OPENCODE", "k")]));
        let names: Vec<&str> = got.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["openrouter", "opencode"]);
        assert_eq!(got[1].base_url, OPENCODE_BASE_URL);
        assert_eq!(got[1].label, "OpenCode Zen");
        // A blank line in .env is not a key.
        assert_eq!(providers_with(env(&[("AGENT_API_KEY_OPENCODE", "  ")])).len(), 1);
    }

    #[test]
    fn opencode_as_the_base_url_uses_the_main_key_and_is_not_listed_twice() {
        let got = providers_with(env(&[("AGENT_BASE_URL", "https://opencode.ai/zen/v1/"), ("AGENT_API_KEY_OPENCODE", "k")]));
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].name.as_str(), got[0].key_env.as_str()), ("opencode", "AGENT_API_KEY"));
    }

    #[test]
    fn a_local_endpoint_is_named_after_its_host() {
        let got = providers_with(env(&[("AGENT_BASE_URL", "http://127.0.0.1:11434/v1")]));
        assert_eq!(got[0].name, "127.0.0.1:11434");
    }

    fn snap(p: &Provider, fetched: u64, ids: &[&str]) -> Snapshot {
        Snapshot { provider: p.clone(), fetched, error: None, entries: ids.iter().map(|i| Entry::bare(i)).collect() }
    }

    #[test]
    fn a_fresh_list_is_not_asked_for_again() {
        let p = providers_with(env(&[]));
        let old = vec![snap(&p[0], 1000, &["a"])];
        let got = refresh(&p, &old, 1000 + FRESH_SECS - 1, false, |_| panic!("fetched a fresh list"));
        assert_eq!(got, old);
    }

    #[test]
    fn a_stale_or_forced_list_is_fetched() {
        let p = providers_with(env(&[]));
        let old = vec![snap(&p[0], 1000, &["a"])];
        let fetch = |_: &Provider| Ok(vec![Entry::bare("b")]);
        assert_eq!(refresh(&p, &old, 1000 + FRESH_SECS, false, fetch)[0].entries[0].id, "b");
        let got = refresh(&p, &old, 1001, true, fetch);
        assert_eq!((got[0].entries[0].id.as_str(), got[0].fetched), ("b", 1001));
    }

    #[test]
    fn an_offline_minute_keeps_the_last_list_and_says_why() {
        let p = providers_with(env(&[]));
        let old = vec![snap(&p[0], 1000, &["a"])];
        let got = refresh(&p, &old, 99_999, false, |_| Err("no answer: could not resolve host".into()));
        assert_eq!(got[0].entries[0].id, "a");
        assert_eq!(got[0].fetched, 1000);
        assert!(got[0].error.as_deref().unwrap().contains("resolve"));
        // And a failed list is retried next time even inside the window.
        let again = refresh(&p, &got, 99_999, false, |_| Ok(vec![Entry::bare("c")]));
        assert_eq!(again[0].entries[0].id, "c");
    }

    #[test]
    fn the_cache_reads_back_what_it_wrote() {
        let p = providers_with(env(&[("AGENT_API_KEY_OPENCODE", "k")]));
        let mut rich = parse(OPENROUTER).unwrap();
        rich.truncate(2);
        let snaps = vec![
            Snapshot { provider: p[0].clone(), fetched: 42, error: None, entries: rich },
            Snapshot { provider: p[1].clone(), fetched: 7, error: Some("HTTP 401".into()), entries: parse(BARE).unwrap() },
        ];
        let text = to_json(&snaps);
        assert!(json::parse(&text).is_ok());
        assert_eq!(from_json(&text, &p), snaps);
        // A provider no longer configured is dropped on the way in.
        assert_eq!(from_json(&text, &p[..1]).len(), 1);
    }
}
