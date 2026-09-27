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

/// Where the auth file lives: beside the chats, in Syn's own data folder
/// (`AGENT_HOME`, or `.agent` beside the `.env`), so one place holds
/// everything this tool keeps. It used to be `.agent` under whatever folder
/// the process started in, which for an installed program is wherever its
/// shortcut says, and a key saved from one shortcut vanished from another.
pub fn auth_path() -> PathBuf {
    crate::chats::home().join("auth.json")
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
        // A key saved from Settings is sealed to this Windows account; one
        // that will not open (another account, another machine) is skipped
        // like any other bad row rather than sent as gibberish.
        "api" => Some(Credential::Api { key: unseal(&f("key")?)? }),
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

// ---- Saving a key from Settings --------------------------------------------
//
// An installed program has no `.env` its user will ever open. Keys typed into
// the console's Settings land here, in the same file every lookup above
// already reads first. On Windows each key is sealed with DPAPI to the
// Windows account that saved it, so the file is useless copied to another
// machine or read by another user; elsewhere the file is readable by its
// owner alone. Neither stops a program running as you: nothing on the
// machine can, and saying otherwise would be a lie.

/// Longest key accepted. Real keys are under 200 characters; this refuses a
/// paste of the wrong thing rather than storing it.
pub const MAX_KEY: usize = 500;

/// Where a provider's key comes from, for Settings to show. Never the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Saved from Settings (or written into the auth file).
    Saved,
    /// Only in the environment, under this variable (usually from `.env`).
    Env(String),
    /// Nowhere.
    Missing,
}

/// Where the key for `provider` comes from, and its last four characters so
/// a person can tell which key it is. Mirrors `resolve`'s order.
pub fn source(provider: &str, key_env: &str) -> (Source, String) {
    let tail = |k: &str| {
        let n = k.chars().count();
        if n <= 8 { String::new() } else { k.chars().skip(n - 4).collect() }
    };
    if let Some(c) = get(provider) {
        return (Source::Saved, tail(c.bearer()));
    }
    match std::env::var(key_env).ok().filter(|k| !k.trim().is_empty()) {
        Some(k) => (Source::Env(key_env.to_string()), tail(&k)),
        None => (Source::Missing, String::new()),
    }
}

/// Save `key` for `provider` (its base URL), replacing any saved before.
pub fn save_key(provider: &str, key: &str) -> Result<(), String> {
    save_key_in(&auth_path(), provider, key)
}

/// Forget the saved key for `provider`. A key in the environment stays.
pub fn forget_key(provider: &str) -> Result<bool, String> {
    forget_key_in(&auth_path(), provider)
}

fn check_key(key: &str) -> Result<String, String> {
    let k = key.trim();
    if k.is_empty() {
        return Err("the key is empty".into());
    }
    if k.chars().count() > MAX_KEY {
        return Err(format!("that is over {MAX_KEY} characters, which is not an API key: paste the key alone"));
    }
    if k.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("an API key has no spaces or line breaks in it: paste the key alone".into());
    }
    Ok(k.to_string())
}

fn save_key_in(path: &std::path::Path, provider: &str, key: &str) -> Result<(), String> {
    if std::env::var(AUTH_CONTENT_ENV).is_ok_and(|d| !d.trim().is_empty()) {
        return Err(format!("keys on this machine come from {AUTH_CONTENT_ENV}, so a saved one would never be read"));
    }
    let key = check_key(key)?;
    let mut all = read_file(path);
    all.insert(normalise(provider), Credential::Api { key });
    write_file(path, &all)
}

fn forget_key_in(path: &std::path::Path, provider: &str) -> Result<bool, String> {
    let mut all = read_file(path);
    let had = all.remove(&normalise(provider)).is_some();
    if had {
        write_file(path, &all)?;
    }
    Ok(had)
}

fn read_file(path: &std::path::Path) -> BTreeMap<String, Credential> {
    std::fs::read_to_string(path).map(|d| parse(&d)).unwrap_or_default()
}

fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// The whole file, written to a temporary name and moved into place, so a
/// crash mid-write leaves the old keys rather than half of the new ones.
fn write_file(path: &std::path::Path, all: &BTreeMap<String, Credential>) -> Result<(), String> {
    let mut doc = String::from("{\n");
    let rows: Vec<String> = all
        .iter()
        .map(|(name, c)| {
            let body = match c {
                Credential::Api { key } => format!("{{\"type\":\"api\",\"key\":{}}}", json_str(&seal(key))),
                Credential::Oauth { access, refresh, expires } => format!(
                    "{{\"type\":\"oauth\",\"access\":{},\"refresh\":{},\"expires\":{expires}}}",
                    json_str(access),
                    json_str(refresh)
                ),
                Credential::WellKnown { key, token } => {
                    format!("{{\"type\":\"wellknown\",\"key\":{},\"token\":{}}}", json_str(key), json_str(token))
                }
            };
            format!("  {}: {body}", json_str(name))
        })
        .collect();
    doc.push_str(&rows.join(",\n"));
    doc.push_str("\n}\n");
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, doc).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot save {}: {e}", path.display()))
}

/// Prefix of a key sealed with DPAPI, so a plain key written by hand into
/// the file still reads as itself.
const SEALED: &str = "dpapi:";

#[cfg(windows)]
fn seal(key: &str) -> String {
    match dpapi::protect(key.as_bytes()) {
        Some(b) => format!("{SEALED}{}", b64::encode(&b)),
        // Sealing failed (no user profile loaded, say): store it the way
        // opencode does rather than lose it.
        None => key.to_string(),
    }
}

#[cfg(not(windows))]
fn seal(key: &str) -> String {
    key.to_string()
}

fn unseal(stored: &str) -> Option<String> {
    let Some(b) = stored.strip_prefix(SEALED) else {
        return Some(stored.to_string());
    };
    #[cfg(windows)]
    {
        String::from_utf8(dpapi::unprotect(&b64::decode(b)?)?).ok()
    }
    #[cfg(not(windows))]
    {
        let _ = b;
        None
    }
}

/// DPAPI, from the Windows system library: the call every Windows program
/// uses to keep a secret for the user who is signed in. Plain FFI into
/// crypt32, which ships with Windows; nothing is added to the build.
#[cfg(windows)]
mod dpapi {
    use std::ffi::c_void;

    #[repr(C)]
    struct Blob {
        len: u32,
        data: *mut u8,
    }

    #[link(name = "crypt32")]
    unsafe extern "system" {
        fn CryptProtectData(
            input: *const Blob,
            desc: *const u16,
            entropy: *const Blob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            out: *mut Blob,
        ) -> i32;
        fn CryptUnprotectData(
            input: *const Blob,
            desc: *mut *mut u16,
            entropy: *const Blob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            out: *mut Blob,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LocalFree(mem: *mut c_void) -> *mut c_void;
    }

    /// Never show a prompt: Syn runs without a window to show one in.
    const UI_FORBIDDEN: u32 = 0x1;
    /// Mixed into every seal, so a blob lifted from this file does not open
    /// through another program's plain DPAPI call.
    const ENTROPY: &[u8] = b"syn/api-key/1";

    fn run(input: &[u8], seal: bool) -> Option<Vec<u8>> {
        let inb = Blob { len: input.len() as u32, data: input.as_ptr() as *mut u8 };
        let ent = Blob { len: ENTROPY.len() as u32, data: ENTROPY.as_ptr() as *mut u8 };
        let mut out = Blob { len: 0, data: std::ptr::null_mut() };
        // SAFETY: every pointer is to a live local or a slice that outlives
        // the call; `out` is filled by the system and freed below with the
        // allocator it came from.
        let ok = unsafe {
            if seal {
                CryptProtectData(&inb, std::ptr::null(), &ent, std::ptr::null_mut(), std::ptr::null_mut(), UI_FORBIDDEN, &mut out)
            } else {
                CryptUnprotectData(&inb, std::ptr::null_mut(), &ent, std::ptr::null_mut(), std::ptr::null_mut(), UI_FORBIDDEN, &mut out)
            }
        };
        if ok == 0 || out.data.is_null() {
            return None;
        }
        // SAFETY: the system says `out.data` holds `out.len` bytes.
        let bytes = unsafe { std::slice::from_raw_parts(out.data, out.len as usize) }.to_vec();
        unsafe { LocalFree(out.data as *mut c_void) };
        Some(bytes)
    }

    pub fn protect(plain: &[u8]) -> Option<Vec<u8>> {
        run(plain, true)
    }

    pub fn unprotect(sealed: &[u8]) -> Option<Vec<u8>> {
        run(sealed, false)
    }
}

/// Base64, standard alphabet with padding: a sealed key is binary and the
/// file is text.
#[cfg_attr(not(windows), allow(dead_code))]
mod b64 {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(b: &[u8]) -> String {
        let mut o = String::with_capacity(b.len().div_ceil(3) * 4);
        for c in b.chunks(3) {
            let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
            o.push(A[(n >> 18) as usize & 63] as char);
            o.push(A[(n >> 12) as usize & 63] as char);
            o.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
            o.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
        }
        o
    }

    pub fn decode(s: &str) -> Option<Vec<u8>> {
        let s = s.trim_end_matches('=');
        let mut o = Vec::with_capacity(s.len() * 3 / 4);
        let (mut acc, mut bits) = (0u32, 0u32);
        for ch in s.bytes() {
            let v = A.iter().position(|&x| x == ch)? as u32;
            acc = (acc << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                o.push((acc >> bits) as u8);
                acc &= (1 << bits) - 1;
            }
        }
        Some(o)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_every_length_and_every_byte() {
        for n in 0..70 {
            let b: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(b64::decode(&b64::encode(&b)).unwrap(), b, "length {n}");
        }
        assert_eq!(b64::encode(b"Man"), "TWFu");
        assert_eq!(b64::encode(b"Ma"), "TWE=");
        assert!(b64::decode("not*base64").is_none());
    }

    fn tmp_auth(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("syn-auth-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("auth.json")
    }

    #[test]
    fn a_saved_key_reads_back_and_is_never_in_the_file_as_typed_on_windows() {
        let p = tmp_auth("save");
        let key = "sk-test-0123456789abcdef";
        save_key_in(&p, "https://example.test/v1/", key).unwrap();
        let back = read_file(&p);
        assert_eq!(back["https://example.test/v1"].bearer(), key, "stored under the normalised provider");
        let text = std::fs::read_to_string(&p).unwrap();
        if cfg!(windows) {
            assert!(!text.contains(key), "sealed to the account, not written as typed: {text}");
            assert!(text.contains(SEALED), "{text}");
        }
        // A second provider keeps the first.
        save_key_in(&p, "https://other.test", "sk-other-000000000").unwrap();
        assert_eq!(read_file(&p).len(), 2);
        assert!(forget_key_in(&p, "https://example.test/v1").unwrap());
        assert!(!forget_key_in(&p, "https://example.test/v1").unwrap(), "nothing left to forget");
        assert_eq!(read_file(&p).keys().collect::<Vec<_>>(), ["https://other.test"]);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn a_paste_that_is_not_a_key_is_refused_and_says_why() {
        let p = tmp_auth("bad");
        assert!(save_key_in(&p, "x", "   ").unwrap_err().contains("empty"));
        assert!(save_key_in(&p, "x", "sk-one sk-two").unwrap_err().contains("no spaces"));
        assert!(save_key_in(&p, "x", &"k".repeat(MAX_KEY + 1)).unwrap_err().contains("not an API key"));
        assert!(!p.exists(), "nothing was written");
        // Surrounding whitespace from a copy is not part of the key.
        save_key_in(&p, "x", "  sk-trimmed-0000000000 \n").unwrap();
        assert_eq!(read_file(&p)["x"].bearer(), "sk-trimmed-0000000000");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn a_key_sealed_elsewhere_is_skipped_not_sent() {
        let doc = r#"{"https://a.test":{"type":"api","key":"dpapi:AAAA"},"https://b.test":{"type":"api","key":"sk-plain"}}"#;
        let all = parse(doc);
        assert!(!all.contains_key("https://a.test"), "a blob that will not open is not a credential");
        assert_eq!(all["https://b.test"].bearer(), "sk-plain", "a key written by hand still reads as itself");
    }

    #[test]
    fn settings_learns_where_a_key_is_from_and_only_its_last_four() {
        // No saved key for an invented provider, and an invented variable.
        let (src, tail) = source("https://nobody.test", "SYN_TEST_NO_SUCH_KEY");
        assert_eq!(src, Source::Missing);
        assert!(tail.is_empty());
    }

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
