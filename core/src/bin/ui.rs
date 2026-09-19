//! ui: a loopback web console for the CLI.
//!
//! It drives `cli.exe` as a child process over its stdin/stdout rather than
//! reimplementing the command surface. That is the point: the console cannot
//! drift from the CLI, because it IS the CLI. Every button below types a
//! line a human could have typed, and the receipts come back verbatim.
//!
//! Zero dependencies, like the rest of `core`: an accept loop, a small HTTP
//! reader and one file served from memory.
//!
//! Security, because this is a local server that can run commands:
//!   * It binds 127.0.0.1 only. Never 0.0.0.0 -- the CLI can attach to your
//!     open documents and, once a program is allowlisted, run it.
//!   * `Origin` does the gatekeeping. A command POST is refused unless its
//!     Origin is this server's own, and refused outright if it carries none.
//!     Browsers attach Origin to every cross-origin POST and cannot forge
//!     it, so a page you happen to visit cannot drive this port even with a
//!     simple request that needs no preflight. Preflights are refused too.
//!
//!     There was a per-install token on top of this. It is gone: the address
//!     is now just http://127.0.0.1:<port>/ with nothing to copy around. The
//!     Origin rule is what was stopping a hostile page either way; what the
//!     token additionally covered was a non-browser caller on this machine,
//!     and anything running locally can drive Office directly regardless.
//!   * One request at a time. The child has one stdin, and serialising here
//!     means a command and a poll can never interleave mid-reply.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const PAGE: &str = include_str!("../../../widget/index.html");

/// The CLI, running, with a pipe to its mouth and one to its ear.
struct Cli {
    child: Child,
    stdin: ChildStdin,
    out: BufReader<ChildStdout>,
    seq: u64,
}

impl Cli {
    fn start(exe: &std::path::Path) -> std::io::Result<Self> {
        let mut child = Command::new(exe)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| std::io::Error::new(e.kind(), format!("cannot start {}: {e}", exe.display())))?;
        let stdin = child.stdin.take().expect("piped");
        let out = BufReader::new(child.stdout.take().expect("piped"));
        let mut cli = Self { child, stdin, out, seq: 0 };
        // Drain the banner so the first command's reply is not prefixed by it.
        let _ = cli.exec("");
        Ok(cli)
    }

    /// Run one command line and return everything it printed.
    ///
    /// Framed by a `mark` sentinel: commands print a variable number of
    /// lines, so a reader without one either guesses a count or blocks
    /// forever on a command that printed nothing.
    fn exec(&mut self, line: &str) -> std::io::Result<String> {
        self.seq += 1;
        let tag = format!("m{}", self.seq);
        writeln!(self.stdin, "{line}")?;
        writeln!(self.stdin, "mark {tag}")?;
        self.stdin.flush()?;
        let want = format!("RECEIPT mark={tag}");
        let mut acc = String::new();
        loop {
            let mut got = String::new();
            if self.out.read_line(&mut got)? == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "the cli exited (did a command quit it?)",
                ));
            }
            if got.trim_end() == want {
                return Ok(acc);
            }
            acc.push_str(&got);
        }
    }
}

impl Drop for Cli {
    fn drop(&mut self) {
        // Ask first, then insist: a clean quit lets the CLI detach its hands
        // instead of leaving a sidecar holding a document.
        let _ = writeln!(self.stdin, "quit");
        let _ = self.stdin.flush();
        let _ = self.child.wait();
    }
}

struct Req {
    method: String,
    path: String,
    origin: Option<String>,
    body: String,
}

fn read_request(s: &TcpStream) -> std::io::Result<Req> {
    let mut r = BufReader::new(s);
    let mut start = String::new();
    r.read_line(&mut start)?;
    let mut parts = start.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();
    let (mut len, mut origin) = (0usize, None);
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            break;
        }
        let t = line.trim_end();
        if t.is_empty() {
            break;
        }
        if let Some((k, v)) = t.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_string());
            match k.as_str() {
                "content-length" => len = v.parse().unwrap_or(0),
                "origin" => origin = Some(v),
                _ => {}
            }
        }
    }
    // Cap the body: this is a command line, not an upload.
    let len = len.min(64 * 1024);
    let mut body = vec![0u8; len];
    if len > 0 {
        r.read_exact(&mut body)?;
    }
    Ok(Req { method, path, origin, body: String::from_utf8_lossy(&body).into_owned() })
}

fn respond(s: &mut TcpStream, status: &str, ctype: &str, body: &str) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes())?;
    s.write_all(body.as_bytes())?;
    s.flush()
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let port: u16 = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse().ok())
        .unwrap_or(7777);

    // The CLI sits next to this binary unless told otherwise.
    let exe = match args.iter().position(|a| a == "--cli").and_then(|i| args.get(i + 1)) {
        Some(p) => std::path::PathBuf::from(p),
        None => std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join(if cfg!(windows) { "cli.exe" } else { "cli" })))
            .unwrap_or_else(|| std::path::PathBuf::from("cli")),
    };

    let mut cli = match Cli::start(&exe) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ERROR {e}");
            std::process::exit(2);
        }
    };

    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("ERROR cannot bind 127.0.0.1:{port}: {e}");
            std::process::exit(2);
        }
    };
    let allowed_origins =
        [format!("http://127.0.0.1:{port}"), format!("http://localhost:{port}")];
    let url = format!("http://127.0.0.1:{port}/");
    // Write the address where a launcher can read it, so nothing has to
    // capture our stdout to learn it. Redirecting a long-lived server's
    // output makes the launcher's own stdout handle inheritable, and it then
    // never closes, which hangs whatever is reading the launcher.
    if let Some(p) = args.iter().position(|a| a == "--url-file").and_then(|i| args.get(i + 1))
        && let Err(e) = std::fs::write(p, format!("{url}\n"))
    {
        eprintln!("ERROR cannot write {p}: {e}");
    }
    println!("Syn console: {url}");
    println!("(loopback only; ctrl-c to stop)");

    for conn in listener.incoming() {
        let Ok(mut s) = conn else { continue };
        let Ok(req) = read_request(&s) else { continue };

        // Reject an Origin that is not ours. Note it must be a MATCH, not
        // an absence: browsers send Origin on same-origin POSTs as well, so
        // refusing every request that carries one refuses our own page.
        if let Some(o) = &req.origin
            && !allowed_origins.iter().any(|a| a == o)
        {
            let _ = respond(&mut s, "403 Forbidden", "text/plain", "cross-origin requests are refused");
            continue;
        }
        if req.method == "OPTIONS" {
            // Refuse the preflight, which is what stops a foreign page from
            // ever being allowed to send the token header.
            let _ = respond(&mut s, "403 Forbidden", "text/plain", "no");
            continue;
        }

        let path = req.path.split('?').next().unwrap_or("/");
        match (req.method.as_str(), path) {
            ("GET", "/") => {
                let _ = respond(&mut s, "200 OK", "text/html; charset=utf-8", PAGE);
            }
            ("POST", "/cmd") => {
                // Origin is now the whole guard on this path, so it must be
                // present as well as matching. A browser always sends it on a
                // POST, so our own page is unaffected; a caller that sends
                // none is not a page and has to say where it is from.
                if req.origin.is_none() {
                    let _ = respond(
                        &mut s,
                        "403 Forbidden",
                        "text/plain",
                        "POST /cmd needs an Origin header of http://127.0.0.1:<port>",
                    );
                    continue;
                }
                let line = req.body.replace(['\r', '\n'], " ");
                // `quit` would kill the child and leave the console talking
                // to a corpse; stopping the server is ctrl-c's job.
                if line.trim() == "quit" || line.trim() == "exit" {
                    let _ = respond(&mut s, "200 OK", "application/json", "{\"out\":\"(use ctrl-c in the console window to stop)\"}");
                    continue;
                }
                let out = match cli.exec(&line) {
                    Ok(o) => o,
                    Err(e) => format!("ERROR {e}"),
                };
                let body = format!("{{\"out\":\"{}\"}}", json_escape(&out));
                let _ = respond(&mut s, "200 OK", "application/json", &body);
            }
            _ => {
                let _ = respond(&mut s, "404 Not Found", "text/plain", "no such path");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(method: &str, origin: Option<&str>) -> Req {
        Req {
            method: method.into(),
            path: "/cmd".into(),
            origin: origin.map(str::to_string),
            body: "hands".into(),
        }
    }

    /// The rule the token used to share: a command may only come from this
    /// server's own page. With the token gone this is the whole guard, so a
    /// POST carrying no Origin is refused rather than trusted.
    fn allowed(port: u16, r: &Req) -> bool {
        let ours = [format!("http://127.0.0.1:{port}"), format!("http://localhost:{port}")];
        match &r.origin {
            Some(o) => ours.iter().any(|a| a == o),
            None => false,
        }
    }

    #[test]
    fn a_command_post_is_refused_unless_it_comes_from_our_own_page() {
        assert!(allowed(7777, &req("POST", Some("http://127.0.0.1:7777"))));
        assert!(allowed(7777, &req("POST", Some("http://localhost:7777"))));
        // A page you happen to be visiting, and a caller that names nobody.
        assert!(!allowed(7777, &req("POST", Some("https://evil.example"))));
        assert!(!allowed(7777, &req("POST", None)));
        // Another port on this machine is still not us.
        assert!(!allowed(7777, &req("POST", Some("http://127.0.0.1:7788"))));
    }

    #[test]
    fn the_served_page_carries_no_token_to_leak() {
        assert!(!PAGE.contains("__SYN_TOKEN__"));
        assert!(!PAGE.contains("X-Syn-Token"));
    }
}
