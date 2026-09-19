//! MCP gateway: expose the 6 primitive ops outward as MCP tools over stdio.
//! Direction: external MCP clients -> this bridge -> in-process Relay for
//! reads/registry, or a queued `office-rpc/1` request for mutating ops until
//! a live hand (COM sidecar) is attached. No dependencies; JSON is assembled
//! by hand and kept flat so any MCP client can parse it.

use crate::bus::Relay;
use crate::ops::{Call, OpOut, ReadArgs, execute};

/// The 6 primitives, exposed 1:1 as MCP tool names.
pub const TOOLS: &[(&str, &str)] = &[
    ("read", "Read a range/para/slide from an open handle. Args: handle, selector."),
    ("write", "Write grid cells or a paragraph. Args: handle, selector, payload."),
    ("format", "Apply key=value styles to a selector. Args: handle, selector, styles."),
    ("struct", "Structural verbs: addSheet/addTable/xfer/chart. Args: handle, verb, payload."),
    ("export", "Write handle out to xlsx/docx/pptx/pdf. Args: handle, format, path."),
    ("undo", "Restore the last snapshot for a handle. Args: handle."),
];

pub fn tools_list_json() -> String {
    let mut s = String::from("{\"tools\":[");
    for (i, (name, desc)) in TOOLS.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!("{{\"name\":\"{name}\",\"description\":\"{desc}\"}}"));
    }
    s.push_str("]}");
    s
}

/// office-rpc/1 method name for a tool. Unknown tools have none.
pub fn method_for(tool: &str) -> Option<&str> {
    TOOLS.iter().any(|(n, _)| *n == tool).then_some(tool)
}

/// Queued request envelope a live hand consumes when attached.
pub fn rpc_request(tool: &str, handle: &str, args_json: &str) -> String {
    format!("{{\"jsonrpc\":\"office-rpc/1\",\"method\":\"{tool}\",\"handle\":\"{handle}\",\"args\":{args_json}}}")
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Human-readable one-liner for an op outcome (also the MCP text content).
pub fn format_out(out: &OpOut) -> String {
    match out {
        OpOut::Grid { sheet, rows, cols } => format!("grid {sheet}: {rows}x{cols}"),
        OpOut::Text { detail } => format!("text: {detail}"),
        OpOut::Count { what, n } => format!("{what}={n}"),
        OpOut::Transfer(t) => format!("xfer {} -> {} ({} rows)", t.from, t.to, t.rows),
        OpOut::Undone { remaining } => format!("undone, {remaining} snapshots left"),
    }
}

pub fn mcp_ok(text: &str) -> String {
    format!("{{\"content\":[{{\"type\":\"text\",\"text\":\"{}\"}}]}}", escape(text))
}

pub fn mcp_err(text: &str) -> String {
    format!("{{\"isError\":true,\"content\":[{{\"type\":\"text\",\"text\":\"{}\"}}]}}", escape(text))
}

/// Dispatch one tool call. Reads + registry run live against the relay;
/// mutating tools return the queued office-rpc request until a hand attaches.
pub fn dispatch(relay: &mut Relay, session: &str, tool: &str, handle: &str, selector: &str) -> String {
    if tool == "registry" {
        return match relay.registry(session) {
            Ok(h) => mcp_ok(&format!("{} open: {}", h.len(), h.join(", "))),
            Err(e) => mcp_err(&e.to_string()),
        };
    }
    let Some(method) = method_for(tool) else {
        return mcp_err(&format!("unknown tool {tool:?}"));
    };
    match method {
        "read" => match execute(relay, session, handle, Call::Read(ReadArgs { selector: selector.into() })) {
            Ok(out) => mcp_ok(&format_out(&out)),
            Err(e) => mcp_err(&e.to_string()),
        },
        _ => mcp_ok(&format!(
            "no live hand attached: queued {}",
            rpc_request(method, handle, &format!("{{\"selector\":\"{}\"}}", escape(selector)))
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{FileContent, FileKind, OpenFile};
    use std::collections::HashMap;

    fn relay1() -> (Relay, String, String) {
        let mut r = Relay::new();
        r.handshake("s", "t");
        let h = "excel:p.xlsx:Sheet1".to_string();
        r.attach("s", h.clone(), OpenFile {
            kind: FileKind::Excel,
            content: FileContent::Excel { sheets: HashMap::from([("Sheet1".into(), vec![vec!["1".into()]])]) },
            styles: HashMap::new(),
        });
        (r, "s".into(), h)
    }

    #[test]
    fn list_exposes_six_tools() {
        let l = tools_list_json();
        for t in ["read", "write", "format", "struct", "export", "undo"] {
            assert!(l.contains(t), "missing {t}");
        }
    }

    #[test]
    fn read_runs_live_write_queues() {
        let (mut r, s, h) = relay1();
        let out = dispatch(&mut r, &s, "read", &h, "Sheet1");
        assert!(out.contains("grid Sheet1"), "{out}");
        let q = dispatch(&mut r, &s, "write", &h, "Sheet1!A1:A1");
        assert!(q.contains("office-rpc/1") && q.contains("no live hand"), "{q}");
    }

    #[test]
    fn unknown_tool_and_bad_handle_are_errors() {
        let (mut r, s, _) = relay1();
        assert!(dispatch(&mut r, &s, "pivot", "h", "s").contains("isError"));
        assert!(dispatch(&mut r, &s, "read", "excel:nope:Sheet1", "Sheet1").contains("isError"));
    }

    #[test]
    fn rpc_envelope_shape() {
        let e = rpc_request("write", "h", "{\"a\":1}");
        assert!(e.contains("\"jsonrpc\":\"office-rpc/1\"") && e.contains("\"method\":\"write\""));
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;
    use crate::ops::TransferReceipt;

    #[test]
    fn method_map_covers_all_tools() {
        for (name, _) in TOOLS {
            assert_eq!(method_for(name), Some(*name));
        }
        assert_eq!(method_for("pivot"), None);
        assert_eq!(method_for(""), None);
    }

    #[test]
    fn format_out_names_every_outcome() {
        assert!(format_out(&OpOut::Grid { sheet: "S".into(), rows: 2, cols: 3 }).contains("2x3"));
        assert!(format_out(&OpOut::Text { detail: "d".into() }).contains('d'));
        assert!(format_out(&OpOut::Count { what: "paras".into(), n: 7 }).contains("paras=7"));
        assert!(format_out(&OpOut::Transfer(TransferReceipt { to: "w".into(), from: "x".into(), rows: 4 })).contains("4 rows"));
        assert!(format_out(&OpOut::Undone { remaining: 2 }).contains('2'));
    }

    #[test]
    fn envelopes_escape_quotes() {
        let ok = mcp_ok("say \"hi\" \\ bye");
        assert!(ok.contains("\\\"hi\\\"") && !ok.contains("isError"));
        assert!(mcp_err("bad \"x\"").contains("isError"));
    }
}
