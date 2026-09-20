//! Saved conversations: the history a chat window lists down its side.
//!
//! One file per conversation, JSON Lines: a header line of metadata, then one
//! line per message. Line-based on purpose — appending a turn is a write to
//! the end, a half-written file loses one message instead of the whole
//! conversation, and reading needs no array parser.
//!
//! Files live under `AGENT_HOME`, defaulting to `.agent/chats` beside the repo.
//! Conversations are the user's, not the project's, so nothing here is ever
//! committed; `.agent/` is gitignored.

use crate::provider::Msg;
use std::path::{Path, PathBuf};

/// What the sidebar needs to draw a row, without reading the whole file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMeta {
    pub id: String,
    pub title: String,
    /// Unix seconds of the last write, for "most recent first".
    pub updated: u64,
    pub turns: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chat {
    pub meta: ChatMeta,
    pub msgs: Vec<Msg>,
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
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

/// Read one JSON string field out of a flat object line.
pub fn field(line: &str, name: &str) -> Option<String> {
    let key = format!("\"{name}\":");
    let i = line.find(&key)?;
    let rest = line[i + key.len()..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut it = rest.chars();
    while let Some(c) = it.next() {
        match c {
            '\\' => match it.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let hex: String = it.by_ref().take(4).collect();
                    // A malformed escape aborts the field rather than
                    // substituting a replacement character, so a corrupt
                    // line is reported missing instead of silently altered.
                    out.push(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)?);
                }
                Some(other) => out.push(other),
                None => return None,
            },
            '"' => return Some(out),
            c => out.push(c),
        }
    }
    None
}

fn num(line: &str, name: &str) -> Option<u64> {
    let key = format!("\"{name}\":");
    let i = line.find(&key)?;
    let rest = line[i + key.len()..].trim_start();
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Serialise one message. The role names match what the UI renders, so the
/// page never has to know about Rust enum shapes.
/// One row per tool call in a saved transcript: `(id, tool, arguments,
/// status)`, ready for [`crate::labels::sentence`].
///
/// The console needs a human sentence per call and must not build one
/// itself. A second implementation of the label table in JavaScript would
/// be a second thing to keep in step with the tool surface, and it would
/// sit outside the tests that stop a newly added verb reaching a human as
/// raw JSON.
///
/// The status is read back out of the observation the loop wrote, because
/// that is the only record a reopened conversation has of how a call went.
pub fn call_labels(msgs: &[Msg]) -> Vec<(String, String, String, crate::labels::Status)> {
    use crate::labels::Status;
    let mut out = Vec::new();
    for (i, m) in msgs.iter().enumerate() {
        let Msg::AssistantCalls(raw) = m else { continue };
        // `parse_tool_calls` wants a whole reply body; the transcript keeps
        // only the array. Wrapping it reuses the one parser rather than
        // adding a second that could disagree with it.
        for call in crate::tools::parse_tool_calls(&format!("\"tool_calls\":{raw}")) {
            let answer = msgs[i + 1..].iter().find_map(|x| match x {
                Msg::Tool { id, content } if *id == call.id => Some(content.as_str()),
                _ => None,
            });
            let status = match answer {
                None => Status::Running,
                Some(a) if a.starts_with("refused:") || a.starts_with("denied by the human:") => Status::Refused,
                Some(a) if a.starts_with("error:") => Status::Failed,
                Some(a) if a.starts_with("not run:") => Status::Stopped,
                Some(_) => Status::Done,
            };
            out.push((call.id, call.name, call.arguments, status));
        }
    }
    out
}

pub fn msg_json(m: &Msg) -> String {
    match m {
        Msg::System(t) => format!("{{\"role\":\"system\",\"text\":\"{}\"}}", esc(t)),
        Msg::User(t) => format!("{{\"role\":\"user\",\"text\":\"{}\"}}", esc(t)),
        Msg::Assistant(t) => format!("{{\"role\":\"assistant\",\"text\":\"{}\"}}", esc(t)),
        Msg::AssistantCalls(raw) => format!("{{\"role\":\"calls\",\"text\":\"{}\"}}", esc(raw)),
        Msg::Tool { id, content } => {
            format!("{{\"role\":\"tool\",\"id\":\"{}\",\"text\":\"{}\"}}", esc(id), esc(content))
        }
    }
}

pub fn msg_from_json(line: &str) -> Option<Msg> {
    let role = field(line, "role")?;
    let text = field(line, "text").unwrap_or_default();
    Some(match role.as_str() {
        "system" => Msg::System(text),
        "user" => Msg::User(text),
        "assistant" => Msg::Assistant(text),
        "calls" => Msg::AssistantCalls(text),
        "tool" => Msg::Tool { id: field(line, "id").unwrap_or_default(), content: text },
        // An unknown role from a newer build is dropped rather than guessed
        // at: replaying it as the wrong role would corrupt the conversation
        // the model sees next turn.
        _ => return None,
    })
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Where conversations live: `$AGENT_HOME`, else `.agent` beside the `.env` this
/// build already knows how to find, else `.agent` under the working directory.
pub fn home() -> PathBuf {
    if let Ok(h) = std::env::var("AGENT_HOME")
        && !h.is_empty()
    {
        return PathBuf::from(h);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let base = crate::config::find_env_file(&cwd)
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or(cwd);
    base.join(".agent")
}

fn dir() -> PathBuf {
    home().join("chats")
}

/// A sortable, filename-safe id. Time-ordered so a directory listing is
/// already in conversation order even without reading the files.
pub fn new_id() -> String {
    format!("c{}-{:04x}", now(), std::process::id() & 0xffff)
}

/// First line of the first user message, which is what every chat app uses
/// and what a human actually recognises in a list.
pub fn title_from(msgs: &[Msg]) -> String {
    for m in msgs {
        if let Msg::User(t) = m {
            let line = t.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
            if !line.is_empty() {
                return if line.chars().count() > 60 {
                    let cut: String = line.chars().take(57).collect();
                    format!("{cut}...")
                } else {
                    line.to_string()
                };
            }
        }
    }
    "New chat".into()
}

pub fn save(chat: &Chat) -> std::io::Result<PathBuf> {
    let d = dir();
    std::fs::create_dir_all(&d)?;
    let path = d.join(format!("{}.jsonl", chat.meta.id));
    let mut out = format!(
        "{{\"v\":1,\"id\":\"{}\",\"title\":\"{}\",\"updated\":{}}}\n",
        esc(&chat.meta.id),
        esc(&chat.meta.title),
        chat.meta.updated
    );
    for m in &chat.msgs {
        out.push_str(&msg_json(m));
        out.push('\n');
    }
    std::fs::write(&path, out)?;
    Ok(path)
}

pub fn load(id: &str) -> std::io::Result<Chat> {
    // An id is a file name, never a path: `../../.env` must not be readable
    // through a chat id that arrives from a web page.
    if id.is_empty() || id.contains(['/', '\\', ':']) || id.contains("..") {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("bad chat id {id:?}")));
    }
    let text = std::fs::read_to_string(dir().join(format!("{id}.jsonl")))?;
    let mut lines = text.lines();
    let head = lines.next().unwrap_or("");
    let msgs: Vec<Msg> = lines.filter(|l| !l.trim().is_empty()).filter_map(msg_from_json).collect();
    let turns = msgs.iter().filter(|m| matches!(m, Msg::User(_))).count();
    Ok(Chat {
        meta: ChatMeta {
            id: field(head, "id").unwrap_or_else(|| id.to_string()),
            title: field(head, "title").unwrap_or_else(|| title_from(&msgs)),
            updated: num(head, "updated").unwrap_or(0),
            turns,
        },
        msgs,
    })
}

/// Every saved conversation, most recently touched first.
pub fn list() -> Vec<ChatMeta> {
    let Ok(rd) = std::fs::read_dir(dir()) else {
        return Vec::new();
    };
    let mut out: Vec<ChatMeta> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| {
            let id = e.path().file_stem()?.to_string_lossy().into_owned();
            load(&id).ok().map(|c| c.meta)
        })
        .collect();
    // Newest first: reverse the key rather than the comparator.
    out.sort_by_key(|m| std::cmp::Reverse(m.updated));
    out
}

pub fn delete(id: &str) -> std::io::Result<()> {
    if id.is_empty() || id.contains(['/', '\\', ':']) || id.contains("..") {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("bad chat id {id:?}")));
    }
    std::fs::remove_file(dir().join(format!("{id}.jsonl")))
}

pub fn meta_json(m: &ChatMeta) -> String {
    format!(
        "{{\"id\":\"{}\",\"title\":\"{}\",\"updated\":{},\"turns\":{}}}",
        esc(&m.id),
        esc(&m.title),
        m.updated,
        m.turns
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_saved_transcript_yields_one_sentence_per_call() {
        use crate::labels::{sentence, Status};
        let raw = r#"[{"id":"c1","type":"function","function":{"name":"read","arguments":"{\"handle\":\"excel:p.xlsx:S1\",\"selector\":\"Sheet1!A1:B2\"}"}},{"id":"c2","type":"function","function":{"name":"struct","arguments":"{\"handle\":\"excel:p.xlsx:S1\",\"verb\":\"addSheet\",\"name\":\"Scorecard\"}"}}]"#;
        let msgs = vec![
            Msg::User("go".into()),
            Msg::AssistantCalls(raw.into()),
            Msg::Tool { id: "c1".into(), content: "grid Sheet1: 2x2".into() },
            Msg::Tool { id: "c2".into(), content: "refused: a sheet named Scorecard exists".into() },
        ];
        let rows = call_labels(&msgs);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].3, Status::Done);
        assert_eq!(rows[1].3, Status::Refused);
        assert_eq!(sentence(&rows[0].1, &rows[0].2, rows[0].3), "Read Sheet1!A1:B2");
        assert_eq!(sentence(&rows[1].1, &rows[1].2, rows[1].3), "Refused to add Scorecard");
    }

    #[test]
    fn a_call_with_no_answer_yet_is_still_running() {
        let raw = r#"[{"id":"c9","type":"function","function":{"name":"read","arguments":"{}"}}]"#;
        let rows = call_labels(&[Msg::AssistantCalls(raw.into())]);
        assert_eq!(rows[0].3, crate::labels::Status::Running);
    }

    #[test]
    fn every_way_the_loop_answers_maps_to_a_status() {
        use crate::labels::Status;
        let raw = r#"[{"id":"c1","type":"function","function":{"name":"read","arguments":"{}"}}]"#;
        for (body, want) in [
            ("grid Sheet1: 2x2", Status::Done),
            ("refused: no such handle", Status::Refused),
            ("denied by the human: not now", Status::Refused),
            ("error: modal dialog or busy app", Status::Failed),
            ("not run: an earlier call in this turn stopped the run", Status::Stopped),
        ] {
            let msgs =
                vec![Msg::AssistantCalls(raw.into()), Msg::Tool { id: "c1".into(), content: body.into() }];
            assert_eq!(call_labels(&msgs)[0].3, want, "{body:?}");
        }
    }

    use super::*;

    #[test]
    fn a_message_round_trips_through_json() {
        let cases = vec![
            Msg::User("hello \"world\"\nsecond line".into()),
            Msg::Assistant("done: 4x4 grid".into()),
            Msg::System("rules".into()),
            Msg::AssistantCalls(r#"[{"id":"c1","function":{"name":"read"}}]"#.into()),
            Msg::Tool { id: "c1".into(), content: "grid Sheet1: 4x4".into() },
        ];
        for m in cases {
            let line = msg_json(&m);
            assert!(!line.contains('\n'), "a message must stay on one line: {line}");
            assert_eq!(msg_from_json(&line), Some(m));
        }
    }

    #[test]
    fn control_characters_survive_a_round_trip() {
        let m = Msg::User("tab\there\u{7}bell".into());
        assert_eq!(msg_from_json(&msg_json(&m)), Some(m));
    }

    #[test]
    fn an_unknown_role_is_dropped_not_guessed() {
        // A file written by a newer build must not have its messages
        // replayed to the model under the wrong role.
        assert_eq!(msg_from_json(r#"{"role":"video","text":"x"}"#), None);
        assert_eq!(msg_from_json("not json"), None);
    }

    #[test]
    fn the_title_is_the_first_real_user_line() {
        let msgs = vec![
            Msg::System("rules".into()),
            Msg::User("\n\n  Read the sheet and report the shape  \nand then stop".into()),
        ];
        assert_eq!(title_from(&msgs), "Read the sheet and report the shape");
        assert_eq!(title_from(&[Msg::System("x".into())]), "New chat");
    }

    #[test]
    fn a_long_title_is_cut_with_an_ellipsis() {
        let long = "x".repeat(200);
        let t = title_from(&[Msg::User(long)]);
        assert_eq!(t.chars().count(), 60);
        assert!(t.ends_with("..."));
    }

    #[test]
    fn a_chat_id_cannot_walk_out_of_its_directory() {
        // The id arrives from a web page; it must name a file, not a path.
        for bad in ["../../.env", "a/b", r"a\b", "c:\\x", ".."] {
            assert!(load(bad).is_err(), "{bad} must be refused");
            assert!(delete(bad).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn save_then_load_keeps_the_conversation() {
        let tmp = std::env::temp_dir().join(format!("agent-chats-{}", std::process::id()));
        // SAFETY: single-threaded test, and the value is restored below.
        unsafe { std::env::set_var("AGENT_HOME", &tmp) };
        let chat = Chat {
            meta: ChatMeta { id: new_id(), title: "Quarterly".into(), updated: now(), turns: 1 },
            msgs: vec![Msg::System("rules".into()), Msg::User("hi".into()), Msg::Assistant("hello".into())],
        };
        save(&chat).unwrap();
        let back = load(&chat.meta.id).unwrap();
        assert_eq!(back.msgs, chat.msgs);
        assert_eq!(back.meta.title, "Quarterly");
        assert_eq!(back.meta.turns, 1);
        assert!(list().iter().any(|m| m.id == chat.meta.id));
        delete(&chat.meta.id).unwrap();
        assert!(load(&chat.meta.id).is_err());
        unsafe { std::env::remove_var("AGENT_HOME") };
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
