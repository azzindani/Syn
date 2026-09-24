//! The CLI's conversation, end to end, against a provider of our own.
//!
//! The agent loop is tested offline in `agent.rs` with a scripted brain,
//! but some behaviour lives only in the REPL that drives it: which runner a
//! turn uses, what survives from one message to the next. Those are tested
//! here by running the real `cli` binary against a loopback HTTP server that
//! answers chat completions from a script.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

/// A chat-completions endpoint that answers each request with the next
/// scripted body, and records what it was sent.
struct Provider {
    url: String,
    sent: Arc<Mutex<Vec<String>>>,
}

fn provider(script: Vec<String>) -> Provider {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/v1", l.local_addr().unwrap().port());
    let sent = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&sent);
    std::thread::spawn(move || {
        let mut script = script.into_iter();
        for conn in l.incoming() {
            let Ok(mut s) = conn else { continue };
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; len];
            let _ = r.read_exact(&mut body);
            log.lock().unwrap().push(String::from_utf8_lossy(&body).into_owned());
            let reply = script.next().unwrap_or_else(|| answer("out of script"));
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    Provider { url, sent }
}

fn call(id: &str, tool: &str, args: &str) -> String {
    let args = args.replace('"', "\\\"");
    format!(
        r#"{{"choices":[{{"finish_reason":"tool_calls","message":{{"role":"assistant","content":null,"tool_calls":[{{"id":"{id}","type":"function","function":{{"name":"{tool}","arguments":"{args}"}}}}]}}}}]}}"#
    )
}

fn answer(text: &str) -> String {
    format!(r#"{{"choices":[{{"finish_reason":"stop","message":{{"role":"assistant","content":"{text}"}}}}]}}"#)
}

/// Feed the CLI these lines and return everything it printed.
fn cli(p: &Provider, lines: &[&str]) -> String {
    let home = std::env::temp_dir().join(format!("syn-cli-turns-{}-{}", std::process::id(), p.url.len()));
    let mut child = Command::new(env!("CARGO_BIN_EXE_cli"))
        .env("AGENT_ENV_FILE", home.join("none.env"))
        .env("AGENT_HOME", &home)
        .env("AGENT_BASE_URL", &p.url)
        .env("AGENT_API_KEY", "test-key")
        // Every slot the same model, so a stopped turn does not walk a
        // fallback chain that the script has no replies for.
        .env("AGENT_MODEL_SMALL", "test/m")
        .env("AGENT_MODEL_STANDARD", "test/m")
        .env("AGENT_MODEL_CODING", "test/m")
        .env("AGENT_MODEL_REASONING", "test/m")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    {
        let mut i = child.stdin.take().unwrap();
        for l in lines {
            writeln!(i, "{l}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    let _ = std::fs::remove_dir_all(&home);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

const READ: &str = r#"{"handle":"excel:plan.xlsx:Sheet1","selector":"Sheet1!A1:B2"}"#;
const OTHER: &str = r#"{"handle":"excel:plan.xlsx:Sheet1","selector":"Sheet1!A1:C3"}"#;

#[test]
fn a_repeated_call_ends_the_turn_but_not_the_conversation() {
    // Turn one: the same read three times, which the gate stops. Turn two:
    // the human asks again and the model does something different. That
    // second turn used to die on its first call with "queue is not
    // running", and every turn after it, because the gate had paused the
    // runner and nothing in the console could resume it.
    let p = provider(vec![
        call("c1", "read", READ),
        call("c2", "read", READ),
        call("c3", "read", READ),
        call("c4", "read", OTHER),
        answer("done"),
    ]);
    let out = cli(&p, &["attach excel plan.xlsx Sheet1", "say read it", "say try a wider range"]);
    let first = out.find("RECEIPT say model=").expect(&out);
    let second = out[first + 1..].find("RECEIPT say model=").map(|i| i + first + 1).expect(&out);
    assert!(out[first..second].contains("STOPPED same op+args 3x"), "turn one should stop on the gate:\n{out}");
    let turn2 = &out[second..];
    // And the turn's one-line account is of this turn, not the conversation.
    assert!(turn2.contains("DID Read 1 range"), "the second turn's summary counted the first turn's calls:\n{out}");
    // One conversation: the second turn's request carries both messages.
    let sent = p.sent.lock().unwrap();
    assert!(sent[3].contains("read it") && sent[3].contains("try a wider range"), "{}", sent[3]);
    assert!(!turn2.contains("queue is not running"), "the second message found the run still paused:\n{out}");
    assert!(turn2.contains("ANSWER done"), "the second turn did not finish:\n{out}");
}

#[test]
fn a_call_refused_while_frozen_never_runs_later() {
    // Three identical reads freeze the run; a fourth, different read is
    // refused. After `resume`, each read must get its own answer. It used
    // to get the refused one's: asked for 4x4, given 3x3, one behind for
    // the rest of the session.
    let p = provider(vec![]);
    let r = |sel: &str| format!("read excel:plan.xlsx:Sheet1 Sheet1!{sel}");
    let out = cli(
        &p,
        &["attach excel plan.xlsx Sheet1", &r("A1:B2"), &r("A1:B2"), &r("A1:B2"), &r("A1:C3"), "resume", &r("A1:D4"), &r("A1:E5")],
    );
    let after = &out[out.find("RECEIPT resumed").expect(&out)..];
    let sizes: Vec<&str> = after.lines().filter_map(|l| l.split("grid Sheet1: ").nth(1)).map(|t| &t[..3]).collect();
    assert_eq!(sizes, ["4x4", "5x5"], "each read after resume must get its own answer:\n{out}");
}
