//! The MCP server as a client meets it: the real binary, over stdio.
//!
//! The unit tests in `core::mcpgate` drive `Server` directly. This drives
//! the process -- the framing, the `.env` handling, the shutdown on a
//! closed stream, and the rule that stdout carries nothing but protocol.
//! A stray `println!` anywhere on the server's path would pass every unit
//! test and break every real client; this is the test that catches it.

use core::json::{self, Value};
use std::io::Write;
use std::process::{Command, Stdio};

fn session(lines: &[&str], extra_env: &[(&str, &str)]) -> (Vec<Value>, String, bool) {
    let home = std::env::temp_dir().join(format!("syn-mcp-test-{}", std::process::id()));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mcpgate"));
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A private home, and no .env: the test sees the defaults, not
        // whatever the machine running it has configured.
        .env("AGENT_HOME", &home)
        .env("AGENT_ENV_FILE", home.join("no-such.env"))
        .env_remove("AGENT_MCP_APPS")
        .env_remove("AGENT_MCP_ROOTS");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("mcpgate runs");
    {
        let mut stdin = child.stdin.take().unwrap();
        for l in lines {
            writeln!(stdin, "{l}").unwrap();
        }
        // Dropping stdin closes the stream: the server must exit on it.
    }
    let out = child.wait_with_output().expect("mcpgate exits when its input closes");
    let stdout = String::from_utf8(out.stdout).unwrap();
    let replies = stdout
        .lines()
        .map(|l| json::parse(l).unwrap_or_else(|e| panic!("stdout carried a non-protocol line {l:?}: {e}")))
        .collect();
    let _ = std::fs::remove_dir_all(&home);
    (replies, String::from_utf8_lossy(&out.stderr).into_owned(), out.status.success())
}

fn text_of(reply: &Value) -> &str {
    reply.at(&["result", "content"]).and_then(Value::as_arr).and_then(|a| a[0].get("text")).and_then(Value::as_str).unwrap_or("")
}

#[test]
fn a_handshake_client_can_list_and_call_and_the_server_exits_cleanly() {
    let (replies, stderr, ok) = session(
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"status","arguments":{}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"read","arguments":{"handle":"excel:x.xlsx:workbook","selector":"S!A1"}}}"#,
        ],
        &[],
    );
    assert!(ok, "exit status; stderr: {stderr}");
    // One answer per request, none for the notification, in order.
    let ids: Vec<String> = replies.iter().map(|r| r.get("id").unwrap().to_json()).collect();
    assert_eq!(ids, ["1", "2", "3", "4"]);
    assert_eq!(replies[0].at(&["result", "protocolVersion"]).and_then(Value::as_str), Some("2025-11-25"));
    assert_eq!(replies[1].at(&["result", "tools"]).and_then(Value::as_arr).map(<[Value]>::len), Some(9));
    assert!(text_of(&replies[2]).contains("OPEN DOCUMENTS"));
    // Nothing is open, and the refusal says what to do about it.
    assert!(text_of(&replies[3]).contains("Call `open`"), "{}", text_of(&replies[3]));
    assert!(stderr.contains("syn mcp"), "human-facing notes go to stderr: {stderr}");
}

#[test]
fn a_modern_client_discovers_then_calls_with_its_version_on_each_request() {
    let meta = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"t","version":"1"}}"#;
    let (replies, _, ok) = session(
        &[
            &format!(r#"{{"jsonrpc":"2.0","id":"a","method":"server/discover","params":{{{meta}}}}}"#),
            &format!(r#"{{"jsonrpc":"2.0","id":"b","method":"tools/call","params":{{{meta},"name":"manual","arguments":{{"topic":"word"}}}}}}"#),
        ],
        &[],
    );
    assert!(ok);
    assert_eq!(replies[0].at(&["result", "resultType"]).and_then(Value::as_str), Some("complete"));
    assert_eq!(replies[1].at(&["result", "resultType"]).and_then(Value::as_str), Some("complete"));
    assert!(text_of(&replies[1]).contains("WORD"), "{}", text_of(&replies[1]));
}

#[test]
fn an_allowlist_from_the_environment_holds_for_open() {
    let (replies, stderr, ok) = session(
        &[r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"open","arguments":{"app":"excel","path":"C:\\x\\plan.xlsx"}}}"#],
        &[("AGENT_MCP_APPS", "word,notanapp")],
    );
    assert!(ok);
    assert!(stderr.contains("notanapp"), "a bad name is reported: {stderr}");
    let t = text_of(&replies[0]);
    assert!(t.contains("allowlist"), "{t}");
}
