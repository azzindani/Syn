//! Config: `.env` loading + model-slot overrides.
//! The model IDs in `provider` are defaults, not policy: a deployment names
//! its own models via `AGENT_MODEL_*` without editing code (the promise made in
//! `provider::model_id`). Values come from the process environment, which
//! `load_env` seeds from the nearest `.env` walking up from the start dir.
//! Real environment always wins over the file, so a shell export overrides a
//! committed default. Std only. Secrets are never returned, logged, or stored
//! in a struct: `AGENT_API_KEY` is read at send time by `provider` alone.

use crate::router::Model;
use std::path::{Path, PathBuf};

/// Env var holding the provider key. Read at send time, never persisted.
pub const API_KEY_ENV: &str = "AGENT_API_KEY";
/// Env var overriding the provider endpoint.
pub const BASE_URL_ENV: &str = "AGENT_BASE_URL";
/// Env var pointing at an explicit `.env` (skips the upward walk).
pub const ENV_FILE_ENV: &str = "AGENT_ENV_FILE";
/// Directories walked upward looking for `.env` before giving up.
const MAX_WALK_UP: usize = 6;

/// What `load_env` applied. Key NAMES only: values may be secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub path: PathBuf,
    pub applied: Vec<String>,
    pub skipped: Vec<String>,
}

/// Parse `.env` text. Blank lines and `#` comments are skipped, a leading
/// `export ` is tolerated, the split is on the FIRST `=`, and one matching
/// pair of surrounding quotes is stripped. Lines without `=` are ignored
/// rather than erroring: a half-edited file must not break startup.
pub fn parse_env(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else { continue };
        let key = k.trim();
        if key.is_empty() {
            continue;
        }
        let v = v.trim();
        let val = match (v.strip_prefix('"').and_then(|s| s.strip_suffix('"')), v.strip_prefix('\'').and_then(|s| s.strip_suffix('\''))) {
            (Some(inner), _) => inner.to_string(),
            (_, Some(inner)) => inner.to_string(),
            _ => v.to_string(),
        };
        out.push((key.to_string(), val));
    }
    out
}

/// Nearest `.env` walking up from one directory. No env lookups: pure.
pub fn find_env_from(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    for _ in 0..MAX_WALK_UP {
        let cur = dir?;
        let candidate = cur.join(".env");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = cur.parent();
    }
    None
}

/// Nearest `.env`: `AGENT_ENV_FILE` if set, else walking up from `start`, else
/// walking up from the executable.
///
/// The executable fallback matters because the working directory is not the
/// project when a launcher starts the binary — a background job, a service,
/// or the widget spawning it. Without it the binary silently ran with no key
/// and default models, which is how this was found.
pub fn find_env_file(start: &Path) -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var(ENV_FILE_ENV) {
        let p = PathBuf::from(explicit);
        return p.is_file().then_some(p);
    }
    find_env_from(start).or_else(|| {
        let exe = std::env::current_exe().ok()?;
        find_env_from(exe.parent()?)
    })
}

/// Load the nearest `.env` into the process environment.
/// Vars already set are left alone and reported in `skipped`.
///
/// Call once at startup before spawning threads: the platform `setenv` this
/// uses is not thread-safe, which is why the edition marks it unsafe.
pub fn load_env(start: &Path) -> Option<Loaded> {
    let path = find_env_file(start)?;
    let text = std::fs::read_to_string(&path).ok()?;
    let mut applied = Vec::new();
    let mut skipped = Vec::new();
    for (k, v) in parse_env(&text) {
        if std::env::var_os(&k).is_some() {
            skipped.push(k);
            continue;
        }
        if v.is_empty() {
            continue; // placeholder row in the committed template
        }
        // SAFETY: startup, single-threaded, before any runner spawns.
        unsafe { std::env::set_var(&k, &v) };
        applied.push(k);
    }
    applied.sort();
    skipped.sort();
    Some(Loaded { path, applied, skipped })
}

/// Env var naming the model for one router slot.
pub fn model_env_key(model: Model) -> &'static str {
    match model {
        Model::Small => "AGENT_MODEL_SMALL",
        Model::Standard => "AGENT_MODEL_STANDARD",
        Model::Coding => "AGENT_MODEL_CODING",
        Model::Reasoning => "AGENT_MODEL_REASONING",
    }
}

/// The endpoint env var for one slot, e.g. `AGENT_BASE_URL_SMALL`.
pub fn base_url_env_key(model: Model) -> &'static str {
    match model {
        Model::Small => "AGENT_BASE_URL_SMALL",
        Model::Standard => "AGENT_BASE_URL_STANDARD",
        Model::Coding => "AGENT_BASE_URL_CODING",
        Model::Reasoning => "AGENT_BASE_URL_REASONING",
    }
}

/// The key env var for one slot, e.g. `AGENT_API_KEY_SMALL`.
pub fn api_key_env_key(model: Model) -> &'static str {
    match model {
        Model::Small => "AGENT_API_KEY_SMALL",
        Model::Standard => "AGENT_API_KEY_STANDARD",
        Model::Coding => "AGENT_API_KEY_CODING",
        Model::Reasoning => "AGENT_API_KEY_REASONING",
    }
}

/// Where one slot's model actually lives: its endpoint, and the name of
/// the env var holding its key.
///
/// Until now there was one endpoint and one key for all four slots, which
/// made the fallback chain a **multi-model** chain and not a
/// multi-provider one. When the provider had a bad minute every slot had
/// it at the same time: one capability run walked nemotron, then ling,
/// then qwen (429), then deepseek, all of them OpenRouter, and died
/// anyway. A chain whose links share a failure mode is one link.
///
/// Per-slot overrides fall back to the global pair, so an existing `.env`
/// keeps working untouched and a second provider is two lines to add.
///
/// This costs no new parsing: OpenRouter, OpenAI, Groq, DeepInfra,
/// Cerebras, Together, Fireworks, llama.cpp, Ollama and LM Studio all
/// speak the OpenAI-compatible shape this module already emits. Providers
/// with their own wire format -- Anthropic's `/v1/messages`, Google's
/// `generateContent` -- need a body and a parser of their own, which is a
/// larger change and not this one.
pub fn endpoint_with(lookup: impl Fn(&str) -> Option<String>, model: Model) -> (String, String) {
    let nonblank = |k: &str| lookup(k).filter(|v| !v.trim().is_empty()).map(|v| v.trim().to_string());
    let url = nonblank(base_url_env_key(model))
        .map(|u| u.trim_end_matches('/').to_string())
        .unwrap_or_else(|| base_url_with(&lookup));
    // The key's NAME, not its value: nothing here reads a secret, and the
    // transport looks it up when it sends. Keeps keys out of every struct
    // that carries a route around.
    let key = if nonblank(api_key_env_key(model)).is_some() {
        api_key_env_key(model).to_string()
    } else {
        API_KEY_ENV.to_string()
    };
    (url, key)
}

/// Resolve one slot's endpoint against the process environment.
pub fn endpoint(model: Model) -> (String, String) {
    endpoint_with(|k| std::env::var(k).ok(), model)
}

/// Resolve a slot against an arbitrary lookup (pure: testable without env).
/// Blank or whitespace-only overrides fall back to the compiled default.
pub fn model_id_with(lookup: impl Fn(&str) -> Option<String>, model: Model) -> String {
    match lookup(model_env_key(model)) {
        Some(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => crate::provider::model_id(model).to_string(),
    }
}

/// Resolve a slot against the process environment.
pub fn model_id(model: Model) -> String {
    model_id_with(|k| std::env::var(k).ok(), model)
}

/// Resolve the endpoint against an arbitrary lookup.
pub fn base_url_with(lookup: impl Fn(&str) -> Option<String>) -> String {
    match lookup(BASE_URL_ENV) {
        Some(v) if !v.trim().is_empty() => v.trim().trim_end_matches('/').to_string(),
        _ => crate::provider::DEFAULT_BASE_URL.to_string(),
    }
}

/// Resolve the endpoint against the process environment.
pub fn base_url() -> String {
    base_url_with(|k| std::env::var(k).ok())
}

/// Env key for an app's sidecar pipe, e.g. `excel` -> `AGENT_PIPE_EXCEL`.
pub fn pipe_env_key(app: &str) -> String {
    format!("AGENT_PIPE_{}", app.to_ascii_uppercase())
}

/// The pipe a sidecar listens on, or None when the deployment has not named
/// one. No compiled-in default: a wrong pipe name fails by connecting to
/// something else, so it is better to have nothing to click.
pub fn pipe_for(app: &str) -> Option<String> {
    std::env::var(pipe_env_key(app)).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Where a Chromium is listening with --remote-debugging-port.
pub const CDP_ENV: &str = "AGENT_CDP";

pub fn cdp_addr() -> Option<String> {
    std::env::var(CDP_ENV).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// True when a provider key is present. Never returns the key itself.
pub fn has_api_key() -> bool {
    std::env::var(API_KEY_ENV).is_ok_and(|k| !k.trim().is_empty())
}

/// One-line startup banner: what resolved where, with no secret material.
pub fn describe() -> String {
    let models: Vec<String> = Model::ALL
        .into_iter()
        .map(|m| format!("{m:?}={}", model_id(m)))
        .collect();
    format!(
        "base={} key={} {}",
        base_url(),
        if has_api_key() { "set" } else { "MISSING" },
        models.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comments_quotes_and_export() {
        let got = parse_env(
            "# comment\n\nexport AGENT_API_KEY=sk-or-v1-abc\nAGENT_MODEL_CODING=\"vendor/model-x\"\nAGENT_MODEL_SMALL='vendor/model-y'\nnoequals\n=novalue\nAGENT_BASE_URL = https://h.test/v1 \n",
        );
        assert_eq!(
            got,
            vec![
                ("AGENT_API_KEY".to_string(), "sk-or-v1-abc".to_string()),
                ("AGENT_MODEL_CODING".to_string(), "vendor/model-x".to_string()),
                ("AGENT_MODEL_SMALL".to_string(), "vendor/model-y".to_string()),
                ("AGENT_BASE_URL".to_string(), "https://h.test/v1".to_string()),
            ]
        );
    }

    #[test]
    fn value_may_contain_equals_and_hash() {
        let got = parse_env("K=a=b=c\nJ=v#notacomment");
        assert_eq!(got[0].1, "a=b=c");
        assert_eq!(got[1].1, "v#notacomment");
    }

    #[test]
    fn model_override_wins_blank_falls_back() {
        let pick = |want: &str, val: &str| {
            let (want, val) = (want.to_string(), val.to_string());
            move |k: &str| (k == want).then(|| val.clone())
        };
        assert_eq!(model_id_with(pick("AGENT_MODEL_CODING", "vendor/code-1"), Model::Coding), "vendor/code-1");
        // trimmed
        assert_eq!(model_id_with(pick("AGENT_MODEL_SMALL", "  v/l  "), Model::Small), "v/l");
        // blank override is not a selection
        assert_eq!(model_id_with(pick("AGENT_MODEL_STANDARD", "   "), Model::Standard), crate::provider::model_id(Model::Standard));
        // unset slot keeps the compiled default
        assert_eq!(model_id_with(|_| None, Model::Reasoning), crate::provider::model_id(Model::Reasoning));
    }

    #[test]
    fn slots_have_distinct_keys() {
        let keys: Vec<&str> = [Model::Small, Model::Standard, Model::Coding, Model::Reasoning].into_iter().map(model_env_key).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len());
        assert!(keys.iter().all(|k| k.starts_with("AGENT_MODEL_")));
    }

    #[test]
    fn base_url_trims_trailing_slash_and_falls_back() {
        assert_eq!(base_url_with(|_| Some("https://h.test/v1/".into())), "https://h.test/v1");
        assert_eq!(base_url_with(|_| Some("  ".into())), crate::provider::DEFAULT_BASE_URL);
        assert_eq!(base_url_with(|_| None), crate::provider::DEFAULT_BASE_URL);
    }

    #[test]
    fn finds_env_walking_up() {
        let root = std::env::temp_dir().join(format!("agent-cfg-{}", std::process::id()));
        let deep = root.join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(root.join(".env"), "AGENT_MODEL_CODING=v/x\n").unwrap();
        assert_eq!(find_env_from(&deep), Some(root.join(".env")));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn walk_up_is_bounded() {
        let root = std::env::temp_dir().join(format!("agent-cfg-deep-{}", std::process::id()));
        // Deeper than MAX_WALK_UP: the file exists but is out of reach, so
        // the walk gives up instead of climbing to the filesystem root.
        let deep = root.join("a/b/c/d/e/f/g/h");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(root.join(".env"), "AGENT_MODEL_CODING=v/x\n").unwrap();
        assert_eq!(find_env_from(&deep), None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_env_file_is_not_an_error() {
        let empty = std::env::temp_dir().join(format!("agent-cfg-none-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        // No .env anywhere under a temp leaf: walk terminates, no panic.
        assert!(load_env(&empty).is_none() || find_env_file(&empty).is_some());
        std::fs::remove_dir_all(&empty).ok();
    }

    #[test]
    fn a_slot_can_live_on_its_own_provider() {
        use crate::router::Model;
        // The whole point: before this, four slots shared one endpoint and
        // one key, so the fallback chain was a multi-MODEL chain. A
        // capability run walked nemotron, ling, qwen (429) and deepseek --
        // all OpenRouter -- and died anyway.
        let env = |k: &str| match k {
            "AGENT_BASE_URL" => Some("https://openrouter.ai/api/v1".to_string()),
            "AGENT_BASE_URL_CODING" => Some("https://api.groq.com/openai/v1/".to_string()),
            "AGENT_API_KEY_CODING" => Some("gsk-whatever".to_string()),
            _ => None,
        };
        // A slot with no override inherits the global pair, so an existing
        // .env keeps working untouched.
        let (url, key) = endpoint_with(env, Model::Small);
        assert_eq!(url, "https://openrouter.ai/api/v1");
        assert_eq!(key, API_KEY_ENV);

        // A slot with its own endpoint and key gets both, trailing slash
        // trimmed so the "/chat/completions" join cannot double up.
        let (url, key) = endpoint_with(env, Model::Coding);
        assert_eq!(url, "https://api.groq.com/openai/v1");
        assert_eq!(key, "AGENT_API_KEY_CODING");

        // The KEY NAME comes back, never the key. Nothing in this module
        // reads a secret; the transport looks it up when it sends.
        assert!(!key.contains("gsk-"));
    }

    #[test]
    fn a_blank_override_is_not_an_override() {
        use crate::router::Model;
        // A .env line left as `AGENT_BASE_URL_STANDARD=` must not point the
        // slot at the empty string and fail every call on it.
        let env = |k: &str| match k {
            "AGENT_BASE_URL" => Some("https://openrouter.ai/api/v1".to_string()),
            "AGENT_BASE_URL_STANDARD" => Some("   ".to_string()),
            "AGENT_API_KEY_STANDARD" => Some(String::new()),
            _ => None,
        };
        let (url, key) = endpoint_with(env, Model::Standard);
        assert_eq!(url, "https://openrouter.ai/api/v1");
        assert_eq!(key, API_KEY_ENV);
    }
}
