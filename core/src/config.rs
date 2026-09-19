//! Config: `.env` loading + model-slot overrides.
//! The model IDs in `provider` are defaults, not policy: a deployment names
//! its own models via `SYN_MODEL_*` without editing code (the promise made in
//! `provider::model_id`). Values come from the process environment, which
//! `load_env` seeds from the nearest `.env` walking up from the start dir.
//! Real environment always wins over the file, so a shell export overrides a
//! committed default. Std only. Secrets are never returned, logged, or stored
//! in a struct: `SYN_API_KEY` is read at send time by `provider` alone.

use crate::router::Model;
use std::path::{Path, PathBuf};

/// Env var holding the provider key. Read at send time, never persisted.
pub const API_KEY_ENV: &str = "SYN_API_KEY";
/// Env var overriding the provider endpoint.
pub const BASE_URL_ENV: &str = "SYN_BASE_URL";
/// Env var pointing at an explicit `.env` (skips the upward walk).
pub const ENV_FILE_ENV: &str = "SYN_ENV_FILE";
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

/// Nearest `.env`: `SYN_ENV_FILE` if set, else walking up from `start`, else
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
        Model::Luna => "SYN_MODEL_LUNA",
        Model::Terra => "SYN_MODEL_TERRA",
        Model::Sol => "SYN_MODEL_SOL",
        Model::Astra => "SYN_MODEL_ASTRA",
    }
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

/// Env key for an app's sidecar pipe, e.g. `excel` -> `SYN_PIPE_EXCEL`.
pub fn pipe_env_key(app: &str) -> String {
    format!("SYN_PIPE_{}", app.to_ascii_uppercase())
}

/// The pipe a sidecar listens on, or None when the deployment has not named
/// one. No compiled-in default: a wrong pipe name fails by connecting to
/// something else, so it is better to have nothing to click.
pub fn pipe_for(app: &str) -> Option<String> {
    std::env::var(pipe_env_key(app)).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Where a Chromium is listening with --remote-debugging-port.
pub const CDP_ENV: &str = "SYN_CDP";

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
            "# comment\n\nexport SYN_API_KEY=sk-or-v1-abc\nSYN_MODEL_SOL=\"vendor/model-x\"\nSYN_MODEL_LUNA='vendor/model-y'\nnoequals\n=novalue\nSYN_BASE_URL = https://h.test/v1 \n",
        );
        assert_eq!(
            got,
            vec![
                ("SYN_API_KEY".to_string(), "sk-or-v1-abc".to_string()),
                ("SYN_MODEL_SOL".to_string(), "vendor/model-x".to_string()),
                ("SYN_MODEL_LUNA".to_string(), "vendor/model-y".to_string()),
                ("SYN_BASE_URL".to_string(), "https://h.test/v1".to_string()),
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
        assert_eq!(model_id_with(pick("SYN_MODEL_SOL", "vendor/code-1"), Model::Sol), "vendor/code-1");
        // trimmed
        assert_eq!(model_id_with(pick("SYN_MODEL_LUNA", "  v/l  "), Model::Luna), "v/l");
        // blank override is not a selection
        assert_eq!(model_id_with(pick("SYN_MODEL_TERRA", "   "), Model::Terra), crate::provider::model_id(Model::Terra));
        // unset slot keeps the compiled default
        assert_eq!(model_id_with(|_| None, Model::Astra), crate::provider::model_id(Model::Astra));
    }

    #[test]
    fn slots_have_distinct_keys() {
        let keys: Vec<&str> = [Model::Luna, Model::Terra, Model::Sol, Model::Astra].into_iter().map(model_env_key).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len());
        assert!(keys.iter().all(|k| k.starts_with("SYN_MODEL_")));
    }

    #[test]
    fn base_url_trims_trailing_slash_and_falls_back() {
        assert_eq!(base_url_with(|_| Some("https://h.test/v1/".into())), "https://h.test/v1");
        assert_eq!(base_url_with(|_| Some("  ".into())), crate::provider::DEFAULT_BASE_URL);
        assert_eq!(base_url_with(|_| None), crate::provider::DEFAULT_BASE_URL);
    }

    #[test]
    fn finds_env_walking_up() {
        let root = std::env::temp_dir().join(format!("syn-cfg-{}", std::process::id()));
        let deep = root.join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(root.join(".env"), "SYN_MODEL_SOL=v/x\n").unwrap();
        assert_eq!(find_env_from(&deep), Some(root.join(".env")));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn walk_up_is_bounded() {
        let root = std::env::temp_dir().join(format!("syn-cfg-deep-{}", std::process::id()));
        // Deeper than MAX_WALK_UP: the file exists but is out of reach, so
        // the walk gives up instead of climbing to the filesystem root.
        let deep = root.join("a/b/c/d/e/f/g/h");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(root.join(".env"), "SYN_MODEL_SOL=v/x\n").unwrap();
        assert_eq!(find_env_from(&deep), None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_env_file_is_not_an_error() {
        let empty = std::env::temp_dir().join(format!("syn-cfg-none-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        // No .env anywhere under a temp leaf: walk terminates, no panic.
        assert!(load_env(&empty).is_none() || find_env_file(&empty).is_some());
        std::fs::remove_dir_all(&empty).ok();
    }
}
