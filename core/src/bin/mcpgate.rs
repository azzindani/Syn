//! mcpgate: stdio bridge exposing the 6 primitive ops as MCP tools.
//! Protocol (one JSON object per line, stdin -> stdout):
//!   {"tool":"list"}                              -> tools_list_json()
//!   {"tool":"read","session":"s","handle":"h","selector":"Sheet1"}
//!   {"tool":"registry","session":"s"}
//! Reads run live against an in-process Relay; mutating tools return the
//! queued office-rpc/1 envelope until a live hand attaches. Attach files
//! first via the harness CLI; this bridge shares no state across processes.

use harness_core::bus::Relay;
use harness_core::mcpgate;
use std::io::BufRead;

fn get(field: &str, line: &str) -> String {
    // Minimal flat-JSON string field extractor (std only).
    let key = format!("\"{field}\"");
    let Some(i) = line.find(&key) else { return String::new() };
    let rest = line[i + key.len()..].trim_start_matches([' ', ':']);
    if !rest.starts_with('"') {
        return String::new();
    }
    let mut out = String::new();
    let mut it = rest[1..].chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            if let Some(e) = it.next() {
                out.push(e);
            }
        } else if c == '"' {
            break;
        } else {
            out.push(c);
        }
    }
    out
}

fn main() {
    let mut relay = Relay::new();
    relay.handshake("cli", "mcpgate");
    let stdin = std::io::stdin();
    for line in stdin.lock().lines().map_while(Result::ok) {
        let tool = get("tool", &line);
        let resp = if tool == "list" || tool.is_empty() && line.trim().is_empty() {
            mcpgate::tools_list_json()
        } else {
            let session = get("session", &line);
            let session = if session.is_empty() { "cli" } else { &session };
            mcpgate::dispatch(&mut relay, session, &tool, &get("handle", &line), &get("selector", &line))
        };
        println!("{resp}");
    }
}
