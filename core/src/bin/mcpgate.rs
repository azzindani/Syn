//! mcpgate: Syn's hands as an MCP server, on stdio.
//!
//! Point any MCP client at the built binary. Claude Desktop, in
//! `claude_desktop_config.json`:
//!
//!   {"mcpServers": {"syn": {"command": "C:\\src\\Syn\\core\\target\\release\\mcpgate.exe"}}}
//!
//! It reads the same `.env` as the rest of Syn -- from the working directory
//! or beside the executable, because a client starts servers wherever it
//! likes -- for the pipe names, the browser address and these:
//!
//!   AGENT_MCP_APPS    only these apps, e.g. excel,word (default: all)
//!   AGENT_MCP_ROOTS   only open files under these folders, `;`-separated
//!   AGENT_MCP_LAUNCH  0 to connect only to helpers already running
//!   AGENT_OFFICE_HOST / AGENT_UIA_HOST   where the helpers are, if not
//!                     in this repository's own Release build
//!
//! The protocol and the tools are in `core::mcpgate`; how a document gets
//! opened is `core::desk`. This file is plumbing.

use core::desk::{Desk, EnvConnector, app_key};
use core::mcpgate::Server;
use std::io::{BufRead, Write};

fn main() {
    // stdout IS the protocol. A stray print is a malformed message and the
    // client drops the connection, so everything human-facing goes to
    // stderr, which clients log and never parse.
    let cwd = std::env::current_dir().unwrap_or_default();
    match core::config::load_env(&cwd) {
        Some(l) => eprintln!("syn mcp: settings from {}", l.path.display()),
        None => eprintln!("syn mcp: no .env found; using the environment as it is"),
    }

    let launch = std::env::var("AGENT_MCP_LAUNCH").map(|v| v.trim() != "0").unwrap_or(true);
    let mut desk = Desk::new("mcp", Box::new(EnvConnector::new(launch)));

    if let Ok(list) = std::env::var("AGENT_MCP_APPS")
        && !list.trim().is_empty()
    {
        // Fail closed: a name that means nothing allows nothing, rather
        // than being skipped and leaving the list wider than intended.
        let mut apps = Vec::new();
        for raw in list.split(',').map(str::trim).filter(|a| !a.is_empty()) {
            match app_key(raw) {
                Some(a) => apps.push(a.to_string()),
                None => eprintln!("syn mcp: AGENT_MCP_APPS names {raw:?}, which is not an app; ignoring it"),
            }
        }
        eprintln!("syn mcp: apps allowed: {}", if apps.is_empty() { "none".into() } else { apps.join(", ") });
        desk.runner.lock_allowlist(apps);
    }
    if let Ok(roots) = std::env::var("AGENT_MCP_ROOTS")
        && !roots.trim().is_empty()
    {
        // `;`, not `,` or `:`: a Windows path has a colon and a folder name
        // can have a comma.
        let roots: Vec<_> = roots.split(';').map(str::trim).filter(|r| !r.is_empty()).map(std::path::PathBuf::from).collect();
        eprintln!("syn mcp: files may be opened under: {}", roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join("; "));
        desk.confine_to(roots);
    }

    // So the Syn console can watch an outside model work. Three lines, so
    // the log is past the banner that marks a process with nothing to do
    // (`live::newest`) the moment the first call lands.
    core::live::begin();
    core::live::append("MCP syn server");
    core::live::append(&format!("MCP pid={}", std::process::id()));
    core::live::append("MCP waiting for a client");

    let mut srv = Server::new(desk);
    srv.mirror_to(core::live::append);

    let mut input = std::io::stdin().lock();
    let mut out = std::io::stdout().lock();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match input.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break, // the client closed the stream: shut down
            Ok(_) => {}
        }
        // Lossy, not strict: one bad byte in a cell's text must not end the
        // session. The parser then refuses the message if it is not JSON.
        let line = String::from_utf8_lossy(&buf);
        if let Some(reply) = srv.handle_line(&line)
            && (writeln!(out, "{reply}").is_err() || out.flush().is_err())
        {
            break; // the client is gone
        }
    }
    // Dropping the server drops the desk, whose connector stops the
    // helpers it started. The applications stay open, with the human's
    // documents in them.
}
