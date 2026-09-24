//! Credentials, stored the way opencode stores them.
//!
//! Ported from `packages/opencode/src/auth/index.ts`. Before this there was
//! one env var, `AGENT_API_KEY`, and a second provider meant a second env var
//! invented on the spot. opencode keeps one file keyed by provider, holding
//! one of three kinds of credential, and everything else asks that file.
//!
//! The three kinds matter because they expire differently:
//!
//! - `api` — a key. Never expires, never refreshes.
//! - `oauth` — access token, refresh token and an expiry. The access token
//!   is the one to send and the one that goes stale; the refresh token is
//!   how a new one is fetched.
//! - `wellknown` — a key and a token issued together by a discovery
//!   endpoint.
//!
//! Three things are deliberately faithful to the original:
//!
//! 1. **One file, keyed by provider**, not one env var per provider. Adding
//!    a provider is a row, not a code change.
//! 2. **An env override for the whole file** (`AGENT_AUTH_CONTENT`, their
//!    `OPENCODE_AUTH_CONTENT`). A container or a CI job has nowhere good to
//!    put a file, and this is the escape hatch that keeps it from having to.
//! 3. **Trailing slashes stripped from the key.** `https://host/v1/` and
//!    `https://host/v1` are the same provider, and storing both is how a
//!    credential goes missing.
//!
//! Secrets are read here and returned to the caller that sends them. They
//! are never logged, never put in a prompt, and never written to the
//! journal — the same rule `config` already holds for `AGENT_API_KEY`.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Env var carrying the whole auth file, for machines with nowhere to put
/// one. opencode's `OPENCODE_AUTH_CONTENT`.
pub const AUTH_CONTENT_ENV: &str = "AGENT_AUTH_CONTENT";

/// One stored credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    /// A plain API key.
    Api { key: String },
    /// An OAuth pair. `expires` is milliseconds since the epoch, 0 when the
    /// provider did not say.
    Oauth { access: String, refresh: String, expires: u64 },
    /// A key and token issued together by a discovery endpoint.
    WellKnown { key: String, token: String },
}

impl Credential {
    /// The string to put in the Authorization header.
    ///
    /// For OAuth that is the **access** token, never the refresh token.
    /// Sending the refresh token is a mistake that reads as an auth failure
    /// and sends you looking in the wrong place.
    pub fn bearer(&self) -> &str {
        match self {
            Credential::Api { key } => key,
            Credential::Oauth { access, .. } => access,
            Credential::WellKnown { token, .. } => token,
        }
    }

    /// Whether an OAuth credential is past its expiry.
    ///
    /// `now_ms` is passed in rather than read from the clock so this stays
    /// pure and a test can sit on either side of the boundary. An `expires`
    /// of 0 means the provider did not say, and a credential we cannot date
    /// is treated as live rather than dead: refusing to send it would lock
    /// the user out of a provider that was working.
    pub fn expired(&self, now_ms: u64) -> bool {
        match self {
            Credential::Oauth { expires, .. } => *expires != 0 && *expires <= now_ms,
            _ => false,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Credential::Api { .. } => "api",
            Credential::Oauth { .. } => "oauth",
            Credential::WellKnown { .. } => "wellknown",
        }
    }
}

/// Normalise a provider key. Trailing slashes are not a distinction.
pub fn normalise(key: &str) -> String {
    key.trim().trim_end_matches('/').to_string()
}

/// Where the auth file lives. Beside the chats, under the repo's own dot
/// directory, so one gitignored place holds everything this tool keeps.
pub fn auth_path() -> PathBuf {
    PathBuf::from(".agent").join("auth.json")
}

/// Parse the auth document: `{"<provider>": {"type": "...", ...}, ...}`.
///
/// Hand-scanned, like every other JSON in `core`. Unknown or malformed
/// entries are skipped rather than failing the whole file — opencode does
/// the same (`Record.filterMap` over a decode that returns an option), and
/// the reason is good: one bad row should not lock you out of every
/// provider you have.
pub fn parse(doc: &str) -> BTreeMap<String, Credential> {
    let mut out = BTreeMap::new();
    let mut rest = doc;
    // Walk `"key": { ... }` pairs at the top level.
    while let Some(q) = rest.find('"') {
        let after = &rest[q + 1..];
        let Some(end) = after.find('"') else { break };
        let name = &after[..end];
        let tail = &after[end + 1..];
        let Some(colon) = tail.find(':') else { break };
        let value = tail[colon + 1..].trim_start();
        if !value.starts_with('{') {
            rest = tail;
            continue;
        }
        let Some(obj) = balanced(value) else { break };
        if let Some(c) = credential_from(obj) {
            out.insert(normalise(name), c);
        }
        rest = &value[obj.len()..];
    }
    out
}

fn credential_from(obj: &str) -> Option<Credential> {
    let f = |k: &str| field(obj, k);
    match f("type")?.as_str() {
        "api" => Some(Credential::Api { key: f("key")? }),
        "oauth" => Some(Credential::Oauth {
            access: f("access")?,
            refresh: f("refresh").unwrap_or_default(),
            expires: f("expires").and_then(|v| v.parse().ok()).unwrap_or(0),
        }),
        "wellknown" => Some(Credential::WellKnown { key: f("key")?, token: f("token")? }),
        _ => None,
    }
}

/// One field from a flat JSON object: string values unquoted, numbers raw.
fn field(obj: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let at = obj.find(&needle)? + needle.len();
    let rest = obj[at..].trim_start().strip_prefix(':')?.trim_start();
    if let Some(s) = rest.strip_prefix('"') {
        let mut out = String::new();
        let mut it = s.chars();
        while let Some(c) = it.next() {
            match c {
                '\\' => out.push(it.next()?),
                '"' => return Some(out),
                _ => out.push(c),
            }
        }
        return None;
    }
    let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    (!n.is_empty()).then_some(n)
}

fn balanced(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for (i, &c) in b.iter().enumerate() {
        if in_str {
            match c {
                _ if esc => esc = false,
                b'\\' => esc = true,
                b'"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Every stored credential: the env override first, then the file.
pub fn all() -> BTreeMap<String, Credential> {
    if let Ok(doc) = std::env::var(AUTH_CONTENT_ENV)
        && !doc.trim().is_empty()
    {
        return parse(&doc);
    }
    std::fs::read_to_string(auth_path()).map(|d| parse(&d)).unwrap_or_default()
}

/// The credential for one provider, by endpoint or by name.
pub fn get(provider: &str) -> Option<Credential> {
    all().remove(&normalise(provider))
}

/// Resolve the key to send for one slot.
///
/// The order is deliberate and is the whole point of the port: the stored
/// credential wins, and the env var remains as the floor. An existing
/// `.env` keeps working with nothing added, and a provider added to
/// `auth.json` needs no new env var invented for it.
pub fn resolve(provider: &str, key_env: &str) -> Option<String> {
    if let Some(c) = get(provider) {
        // An expired OAuth token is not a credential. Sending it produces
        // a 401 that reads like a bad key, which is the wrong place to go
        // looking.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if !c.expired(now) {
            return Some(c.bearer().to_string());
        }
    }
    std::env::var(key_env).ok().filter(|k| !k.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"{
      "https://openrouter.ai/api/v1": {"type":"api","key":"sk-or-v1-abc"},
      "https://api.anthropic.com": {"type":"oauth","access":"at-1","refresh":"rt-1","expires":1700000000000},
      "https://example.test": {"type":"wellknown","key":"kid","token":"tok"},
      "broken": {"type":"nonsense"}
    }"#;

    #[test]
    fn the_three_kinds_round_trip() {
        let all = parse(DOC);
        assert_eq!(all.len(), 3, "the malformed row is skipped, not fatal: {all:?}");
        assert_eq!(all["https://openrouter.ai/api/v1"].bearer(), "sk-or-v1-abc");
        // OAuth sends the ACCESS token. Sending the refresh token reads as
        // an auth failure and sends you looking in the wrong place.
        assert_eq!(all["https://api.anthropic.com"].bearer(), "at-1");
        assert_eq!(all["https://example.test"].bearer(), "tok");
        assert_eq!(all["https://api.anthropic.com"].kind(), "oauth");
    }

    #[test]
    fn one_bad_row_does_not_lock_you_out_of_the_others() {
        // opencode filters undecodable entries rather than failing the
        // file, and the reason is worth keeping: a provider added wrongly
        // must not take the working ones down with it.
        let all = parse(r#"{"a":{"type":"api","key":"k"},"b":{"type":"api"},"c":12}"#);
        assert_eq!(all.len(), 1);
        assert!(all.contains_key("a"));
    }

    #[test]
    fn a_trailing_slash_is_not_a_different_provider() {
        assert_eq!(normalise("https://host/v1/"), "https://host/v1");
        assert_eq!(normalise(" https://host/v1// "), "https://host/v1");
        let all = parse(r#"{"https://host/v1/":{"type":"api","key":"k"}}"#);
        assert!(all.contains_key("https://host/v1"), "{all:?}");
    }

    #[test]
    fn an_expired_oauth_token_is_not_a_credential() {
        let live = Credential::Oauth { access: "a".into(), refresh: "r".into(), expires: 2_000 };
        assert!(!live.expired(1_999));
        assert!(live.expired(2_000));
        // A credential the provider gave no expiry for is treated as live:
        // refusing to send it would lock the user out of something that
        // was working.
        let undated = Credential::Oauth { access: "a".into(), refresh: "r".into(), expires: 0 };
        assert!(!undated.expired(u64::MAX));
        // And a plain key never expires.
        assert!(!Credential::Api { key: "k".into() }.expired(u64::MAX));
    }
}
