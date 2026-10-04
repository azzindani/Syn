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
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PAGE: &str = include_str!("../../../widget/index.html");

/// Which page this console serves, so a tab left open across a restart
/// onto a newer build loads it rather than running the old one against the
/// new console. A hash of the page itself (FNV-1a): it changes exactly when
/// the page does.
fn page_version() -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in PAGE.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Whether this console's own child is mid-turn. Progress itself comes
/// from `core::live`, which every CLI writes to disk: a run started from a
/// terminal, a pipe or a second window used to be invisible here, because
/// the only thing this server could see was the child it had spawned
/// itself.
#[derive(Default)]
struct Live {
    busy: AtomicBool,
    /// A turn holds the child: one of `TURNS` is running. Not the lock
    /// itself -- any quick command holds that for a moment, and a picker's
    /// `slots` answered from the copy because a `hands` happened to be in
    /// flight showed the model from before the one just chosen.
    turn: AtomicBool,
}

/// Whether the run behind a log is going. For this console's own child it
/// is known exactly: a turn holds it or not. The log's age was used for
/// every log, and the page's own queries keep the child's log fresh, so a
/// console doing nothing read as working for as long as a page was open --
/// found live, a page showing "Working" for good after the console under it
/// was restarted mid-run. Age stays the answer for other processes' logs (a
/// terminal, an MCP client), where it is the only one there is.
fn running_in(src: &std::path::Path, own: Option<&std::path::Path>, live: &Live) -> bool {
    if own == Some(src) { live.turn.load(Ordering::SeqCst) } else { core::live::active(src) }
}

/// The commands that run the agent, and so hold the child for minutes.
const TURNS: &[&str] = &["say", "do", "approve", "deny", "replay"];

fn is_turn(line: &str) -> bool {
    TURNS.contains(&line.split_whitespace().next().unwrap_or(""))
}

/// What a page asks in order to draw itself, and nothing that changes
/// anything. The child reads one line at a time, so during a turn every
/// command queues behind it -- and a page opened or reloaded mid-run sent
/// `slots` first, waited the whole turn for the answer, and stayed blank:
/// no sidebar, no thread, no live view, for as long as the run took.
const READS: &[&str] = &["slots", "keys", "workspace", "hands", "chat list", "chat msgs"];

/// The last answer the child gave to each of `READS`. Taken fresh as a turn
/// starts, since the state it starts from is what a page opened during it
/// should see.
#[derive(Default)]
struct Seen(Mutex<std::collections::HashMap<String, String>>);

impl Seen {
    fn get(&self, line: &str) -> Option<String> {
        self.0.lock().ok()?.get(line).cloned()
    }
    fn put(&self, line: &str, out: &str) {
        if let Ok(mut m) = self.0.lock() {
            m.insert(line.to_string(), out.to_string());
        }
    }
    /// Ask the child every read again. Called holding its lock.
    fn take(&self, c: &mut Cli) {
        for r in READS {
            if let Ok(out) = c.exec(r) {
                self.put(r, &out);
            }
        }
    }
}

/// A read's answer while a turn holds the child, from what it said as the
/// turn began. None when no turn is running (ask the child: anything else
/// in its way is over in moments) or it was never asked (queue, as before:
/// there is nothing honest to answer with).
fn busy_read(live: &Live, seen: &Seen, line: &str) -> Option<String> {
    if !READS.contains(&line) || !live.turn.load(Ordering::SeqCst) {
        return None;
    }
    seen.get(line)
}

/// The CLI, running, with a pipe to its mouth and one to its ear.
struct Cli {
    child: Child,
    stdin: ChildStdin,
    out: BufReader<ChildStdout>,
    seq: u64,
}

impl Cli {
    fn start(exe: &std::path::Path) -> std::io::Result<Self> {
        // The CLI watches for this console to go (`tether::watch_parent`):
        // killed with Stop-Process, the console used to leave its CLI
        // finishing a turn for nobody, and that CLI's helpers with it.
        let mut child = Command::new(exe)
            .env(core::tether::PARENT_ENV, std::process::id().to_string())
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
    /// What a browser sends when EventSource reconnects on its own: the
    /// `id:` of the last frame it saw.
    last_event_id: Option<String>,
    /// The Host header: which name the browser thinks it is talking to.
    host: Option<String>,
    body: String,
    /// The body was over the cap and was not read.
    too_big: bool,
}

/// The most a request body may hold: a command line, not an upload.
const MAX_BODY: usize = 64 * 1024;

/// Whether a request was addressed to this console by a name that can only
/// mean this machine.
///
/// The Origin check covers POSTs, and a browser sends no Origin on a plain
/// GET. A page on a domain whose DNS answer is switched to 127.0.0.1 after
/// it loads (DNS rebinding) is, to the browser, the same origin as this
/// console, and could read `/events` and `/stream` -- the live transcript,
/// cell values included. Its requests still carry its own name in Host,
/// which is what this refuses.
fn host_ok(port: u16, r: &Req) -> bool {
    r.host.as_deref().is_some_and(|h| {
        let h = h.trim().to_ascii_lowercase();
        h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}")
    })
}

fn read_request(s: &TcpStream) -> std::io::Result<Req> {
    let mut r = BufReader::new(s);
    let mut start = String::new();
    r.read_line(&mut start)?;
    let mut parts = start.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();
    let (mut len, mut origin, mut last_event_id, mut host) = (0usize, None, None, None);
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
                "last-event-id" => last_event_id = Some(v),
                "host" => host = Some(v),
                _ => {}
            }
        }
    }
    // Refused, not cut: a long message pasted into the composer used to
    // arrive as its first 64 KB and run as if that were all of it.
    if len > MAX_BODY {
        return Ok(Req { method, path, origin, last_event_id, host, body: String::new(), too_big: true });
    }
    let mut body = vec![0u8; len];
    if len > 0 {
        r.read_exact(&mut body)?;
    }
    Ok(Req { method, path, origin, last_event_id, host, body: String::from_utf8_lossy(&body).into_owned(), too_big: false })
}

/// One query parameter, by exact name. `split_once("since=")` also matched
/// inside any longer name ending in it.
fn param<'a>(path: &'a str, key: &str) -> Option<&'a str> {
    path.split_once('?')?.1.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v))
}

/// The most of an oversized upload that is read and thrown away after the
/// refusal. A paste is a few megabytes at most; a client sending more than
/// this is not going to see the refusal, and that is fine.
const DRAIN_CAP: usize = 16 * 1024 * 1024;

/// Refuse a body over the cap in words the page can show. Closing the
/// connection with the rest of the upload unread makes the OS reset it, and
/// on Windows the browser then reports "Failed to fetch" and never shows the
/// 413: a 70 KB paste read as "Console unreachable" (the audit caught it; the
/// Linux sandbox delivered the response anyway and never saw it). So the
/// response is finished with a FIN first, and the upload is read to the end.
fn refuse_too_big(s: &mut TcpStream) {
    let _ = respond(
        s,
        "413 Content Too Large",
        "text/plain",
        &format!("the message is over {} KB: send it in parts, or put it in a file and name the file", MAX_BODY / 1024),
    );
    let _ = s.shutdown(std::net::Shutdown::Write);
    let mut sink = [0u8; 8192];
    let mut left = DRAIN_CAP;
    while left > 0 {
        match s.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(n) => left = left.saturating_sub(n),
        }
    }
}

/// Show the console as an application window and stop Syn when that window
/// is closed (see `core::appwin`). True if a window was started.
fn open_app_window(url: &str) -> bool {
    // Beside the program, with the chats and keys: `.agent` is what the
    // launcher's working directory holds.
    let profile = std::env::current_dir().unwrap_or_default().join(".agent").join("app-profile");
    core::appwin::open(url, &profile, true)
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

    // The same .env the CLI child reads, so this process knows which
    // providers to ask for a model list. A shell export still wins, and the
    // child inherits exactly what it would have loaded itself.
    let _ = core::config::load_env(&std::env::current_dir().unwrap_or_default());
    let catalog = Arc::new(Mutex::new(core::catalog::to_json(&core::catalog::load())));
    keep_catalog_fresh(Arc::clone(&catalog));

    let cli = match Cli::start(&exe) {
        Ok(c) => Arc::new(Mutex::new(c)),
        Err(e) => {
            eprintln!("ERROR {e}");
            std::process::exit(2);
        }
    };
    let live = Arc::new(Live::default());
    let seen = Arc::new(Seen::default());
    // The log this console's own child writes, which the view follows
    // while a turn the person started here is running.
    let own_log_cell = Arc::new(Mutex::new(cli.lock().ok().map(|c| core::live::log_of(c.child.id()))));
    // The child's pid outside its lock, so `/stop` can reach a turn that
    // holds the lock for as long as it runs.
    let child_pid = Arc::new(AtomicU32::new(cli.lock().map(|c| c.child.id()).unwrap_or(0)));
    // A run in another process writes its own log; `--tail` pins the
    // console to one file rather than following whichever is newest.
    let pinned: Option<std::path::PathBuf> = args
        .iter()
        .position(|a| a == "--tail")
        .and_then(|i| args.get(i + 1))
        .map(std::path::PathBuf::from);
    if let Some(p) = &pinned {
        println!("watching: {}", p.display());
    }

    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("ERROR cannot bind 127.0.0.1:{port}: {e}");
            std::process::exit(2);
        }
    };
    // The port actually bound: `--port 0` asks the system for a free one,
    // and every check below compares against what the browser will use.
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
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
    println!("the agent console: {url}");
    println!("(loopback only; ctrl-c to stop)");
    // `--open`: a double-clicked copy has no script around it to open the
    // page, and a console window holding an address is not an app. On
    // Windows the page gets a window of its own (below); anywhere that
    // cannot, or when it fails, it goes to the default browser as before.
    if args.iter().any(|a| a == "--open") && !open_app_window(&url) {
        let _ = if cfg!(windows) {
            Command::new("cmd").args(["/C", "start", "", &url]).spawn()
        } else if cfg!(target_os = "macos") {
            Command::new("open").arg(&url).spawn()
        } else {
            Command::new("xdg-open").arg(&url).spawn()
        };
    }

    // One thread per connection. The child still has one stdin, so /cmd
    // serialises on the mutex exactly as before -- but a turn that takes
    // twenty minutes no longer blocks the accept loop, which is what made
    // polling for progress impossible.
    for conn in listener.incoming() {
        let Ok(s) = conn else { continue };
        let cli = Arc::clone(&cli);
        let live = Arc::clone(&live);
        let seen = Arc::clone(&seen);
        let catalog = Arc::clone(&catalog);
        let pinned = pinned.clone();
        let own_log_cell = Arc::clone(&own_log_cell);
        let own_log: Option<std::path::PathBuf> = own_log_cell.lock().ok().and_then(|g| g.clone());
        let child_pid = Arc::clone(&child_pid);
        let exe = exe.clone();
        let allowed_origins = allowed_origins.clone();
        std::thread::spawn(move || {
            let mut s = s;
            // A connection that never finishes its request must not hold a
            // thread for ever. Only the request is read with this; a stream
            // answered below only writes.
            let _ = s.set_read_timeout(Some(Duration::from_secs(15)));
            let Ok(req) = read_request(&s) else { return };
            if !host_ok(port, &req) {
                let _ = respond(&mut s, "421 Misdirected Request", "text/plain", "this console answers to 127.0.0.1 and localhost only");
                return;
            }
            if req.too_big {
                refuse_too_big(&mut s);
                return;
            }

        // Reject an Origin that is not ours. Note it must be a MATCH, not
        // an absence: browsers send Origin on same-origin POSTs as well, so
        // refusing every request that carries one refuses our own page.
        if let Some(o) = &req.origin
            && !allowed_origins.iter().any(|a| a == o)
        {
            let _ = respond(&mut s, "403 Forbidden", "text/plain", "cross-origin requests are refused");
            return;
        }
        if req.method == "OPTIONS" {
            // Refuse the preflight, which is what stops a foreign page from
            // ever being allowed to send the token header.
            let _ = respond(&mut s, "403 Forbidden", "text/plain", "no");
            return;
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
                    return;
                }
                let line = req.body.replace(['\r', '\n'], " ");
                // `quit` would kill the child and leave the console talking
                // to a corpse; stopping the server is ctrl-c's job.
                if line.trim() == "quit" || line.trim() == "exit" {
                    let _ = respond(&mut s, "200 OK", "application/json", "{\"out\":\"(use ctrl-c in the console window to stop)\"}");
                    return;
                }
                let key = line.trim();
                if let Some(out) = busy_read(&live, &seen, key) {
                    let body = format!("{{\"out\":\"{}\"}}", json_escape(&out));
                    let _ = respond(&mut s, "200 OK", "application/json", &body);
                    return;
                }
                live.busy.store(true, Ordering::SeqCst);
                let out = match cli.lock() {
                    Ok(mut c) => {
                        let turn = is_turn(key);
                        if turn {
                            seen.take(&mut c);
                            live.turn.store(true, Ordering::SeqCst);
                        }
                        let out = match c.exec(&line) {
                            Ok(o) => {
                                if READS.contains(&key) {
                                    seen.put(key, &o);
                                }
                                o
                            }
                            Err(e) => format!("ERROR {e}"),
                        };
                        if turn {
                            live.turn.store(false, Ordering::SeqCst);
                        }
                        out
                    }
                    Err(_) => "ERROR the console lock is poisoned; restart the server".to_string(),
                };
                live.busy.store(false, Ordering::SeqCst);
                let body = format!("{{\"out\":\"{}\"}}", json_escape(&out));
                let _ = respond(&mut s, "200 OK", "application/json", &body);
            }
            // What the page polls while a turn is running. Deliberately
            // not behind the Origin-required guard that /cmd has: it changes
            // nothing, and a reader that cannot see progress is the bug.
            // Server-sent events: the run, pushed, instead of the page
            // asking every 700ms and being told "nothing yet" most of the
            // time. Polling made a fast step look slow -- a read that took
            // 40ms could sit invisible for the better part of a second,
            // and a burst of eight calls in one turn arrived as one clump
            // rather than eight rows appearing one after another.
            //
            // The server still watches the file, because that is how a run
            // in another process makes itself visible at all. What changes
            // is who waits: a 60ms loop here holding one open connection,
            // rather than a request every 700ms and a page that cannot
            // tell "quiet" from "not asked yet".
            ("GET", "/stream") => {
                // Where to start: the page says which run it is on and how
                // many of its lines it has drawn. A browser reconnecting by
                // itself says the same thing in `Last-Event-ID`, which every
                // frame carries as `<run>/<line>`; that wins, because it is
                // newer than the URL the stream was first opened with. The
                // stream used to start from `since` alone, and a reconnect
                // replayed the run from its first line.
                let mut since = param(&req.path, "since").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
                let mut asked = param(&req.path, "source").unwrap_or("").to_string();
                if let Some((src, n)) = req.last_event_id.as_deref().and_then(|v| v.rsplit_once('/'))
                    && let Ok(n) = n.parse::<usize>()
                {
                    asked = src.to_string();
                    since = n;
                }
                // `retry:` is how long a browser waits before reconnecting
                // on its own.
                let head = concat!(
                    "HTTP/1.1 200 OK\r\n",
                    "Content-Type: text/event-stream\r\n",
                    "Cache-Control: no-store\r\n",
                    "Connection: keep-alive\r\n",
                    "X-Accel-Buffering: no\r\n\r\n",
                    "retry: 2000\n\n",
                );
                if s.write_all(head.as_bytes()).is_err() {
                    return;
                }
                let _ = s.flush();
                // A write to a browser that has navigated away blocks until
                // the socket notices; a timeout turns that into an error
                // this loop can leave on.
                let _ = s.set_write_timeout(Some(Duration::from_secs(5)));

                let name_of = |p: &Option<std::path::PathBuf>| {
                    p.as_ref().and_then(|p| p.file_name()).and_then(|f| f.to_str()).unwrap_or("").to_string()
                };
                let pick = |current: Option<&std::path::Path>| {
                    pinned.clone().or_else(|| {
                        core::live::follow(current, own_log.as_deref(), live.busy.load(Ordering::SeqCst))
                    })
                };
                // The run to show, chosen the way `/events` chooses, from
                // the one the page is on. The cursor only means something
                // for that run: if the choice is a different one, it starts
                // from the top. It used to keep the old run's line count
                // and read the new run from the middle.
                let mut src = pick(core::live::named(&asked).as_deref());
                let mut source = name_of(&src);
                if source != asked {
                    since = 0;
                }
                let mut tail = src.as_deref().map(|p| core::live::Tail::at(p, since));
                // Name the run, so a page on a different one resets. Naming
                // it is not the same as rewinding to the start of it.
                let at = tail.as_ref().map(core::live::Tail::n).unwrap_or(0);
                // `console` says which console this is, so a page that lost
                // the stream can tell a blip (same console: carry on from
                // its cursor) from a restart (a new one: start over).
                let ev = format!(
                    "event: source\ndata: {{\"source\":\"{}\",\"since\":{at},\"console\":{},\"page\":\"{}\"}}\n\n",
                    json_escape(&source),
                    std::process::id(),
                    page_version()
                );
                if s.write_all(ev.as_bytes()).is_err() || s.flush().is_err() {
                    return;
                }
                let mut beat = Instant::now();
                let mut looked = Instant::now();
                // The last state this stream told the page.
                let mut said: Option<(bool, bool)> = None;
                loop {
                    // Which run to show is a directory scan; twice a second
                    // is plenty for a hand-over that happens once a run.
                    // The lines themselves are read every pass.
                    if looked.elapsed() >= Duration::from_millis(500) {
                        looked = Instant::now();
                        let next = pick(src.as_deref());
                        let name = name_of(&next);
                        // A different run took over. Tell the page, so it
                        // starts that one from the top rather than splicing
                        // it onto the tail of the last.
                        if name != source {
                            source = name;
                            src = next;
                            tail = src.as_deref().map(|p| core::live::Tail::at(p, 0));
                            let ev = format!("event: source\ndata: {{\"source\":\"{}\"}}\n\n", json_escape(&source));
                            if s.write_all(ev.as_bytes()).is_err() {
                                return;
                            }
                        }
                    }
                    let lines = tail.as_mut().map(core::live::Tail::read).unwrap_or_default();
                    // One frame per line, each carrying where it leaves the
                    // cursor, so a page (or a browser reconnecting) always
                    // knows exactly what it has. An SSE `data:` field may
                    // not carry a newline, and these lines never do.
                    let mut n = tail.as_ref().map(core::live::Tail::n).unwrap_or(0) - lines.len();
                    for line in &lines {
                        n += 1;
                        let ev = format!(
                            "id: {}/{n}\ndata: {{\"line\":\"{}\",\"n\":{n}}}\n\n",
                            json_escape(&source),
                            json_escape(line)
                        );
                        if s.write_all(ev.as_bytes()).is_err() {
                            return;
                        }
                    }
                    let busy = live.busy.load(Ordering::SeqCst);
                    let running = src.as_deref().map(|p| running_in(p, own_log.as_deref(), &live)).unwrap_or(false);
                    // Also when it changes with nothing printed: a turn that
                    // ends without a last line -- killed, or its CLI gone --
                    // used to leave the page on "Working" for good.
                    let changed = said != Some((busy, running));
                    if !lines.is_empty() || changed {
                        said = Some((busy, running));
                        let ev =
                            format!("event: state\ndata: {{\"busy\":{busy},\"running\":{running}}}\n\n");
                        if s.write_all(ev.as_bytes()).is_err() || s.flush().is_err() {
                            return;
                        }
                    }
                    // A comment frame every 15s. Without it an idle stream
                    // is indistinguishable from a dead one, to a proxy and
                    // to the browser's own idle timer.
                    if beat.elapsed() >= Duration::from_secs(15) {
                        beat = Instant::now();
                        let ev = format!(": beat busy={busy} running={running}\n\n");
                        if s.write_all(ev.as_bytes()).is_err() || s.flush().is_err() {
                            return;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(60));
                }
            }
            // The model catalog, from memory: the picker opens instantly
            // and never waits on a provider. What is in memory is kept
            // fresh by `keep_catalog_fresh`.
            ("GET", "/models") => {
                // From the cache file each time rather than the string kept
                // at the last refresh: `ready` says whether a key is set now,
                // and a key saved in Settings a moment ago has to count
                // without waiting a quarter of an hour for the next refresh.
                let snaps = core::catalog::load();
                let body = if snaps.is_empty() {
                    catalog.lock().map(|c| c.clone()).unwrap_or_default()
                } else {
                    core::catalog::to_json(&snaps)
                };
                let _ = respond(&mut s, "200 OK", "application/json", &body);
            }
            // The picker's refresh button: ask every provider now. A POST
            // behind the same Origin rule as /cmd, because it makes this
            // machine send requests.
            // Stop the turn that is running. Not through `/cmd`: a turn holds
            // the child's one stdin, so a stop sent there queued behind the
            // very turn it was meant to end -- which is how a run stuck on
            // Excel left the page with no way out. Asked first (the CLI
            // looks for the flag between steps, while it waits, and while
            // a reply streams); when it cannot look, because it is inside
            // a call that is not coming back, the child is restarted.
            ("POST", "/stop") => {
                if req.origin.is_none() {
                    let _ = respond(&mut s, "403 Forbidden", "text/plain", "POST /stop needs an Origin header");
                    return;
                }
                if !live.busy.load(Ordering::SeqCst) {
                    let _ = respond(&mut s, "200 OK", "application/json", "{\"stopped\":\"idle\"}");
                    return;
                }
                let pid = child_pid.load(Ordering::SeqCst);
                let _ = core::stop::request(pid);
                let settled = |secs: u64| {
                    let until = Instant::now() + Duration::from_secs(secs);
                    while Instant::now() < until {
                        if !live.busy.load(Ordering::SeqCst) {
                            return true;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    false
                };
                if settled(4) {
                    let _ = respond(&mut s, "200 OK", "application/json", "{\"stopped\":\"clean\"}");
                    return;
                }
                // Not coming back by itself. The whole tree goes: curl, and
                // any helper this CLI started, which is the thing stuck on
                // the application -- on Windows through taskkill's /T, on
                // Linux because each was bound to the CLI (`tether`) and is
                // signalled when it dies. The application and its documents
                // stay; a helper never closes what it did not open.
                let _ = if cfg!(windows) {
                    Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).output()
                } else {
                    Command::new("kill").args(["-9", &pid.to_string()]).output()
                };
                // The `/cmd` thread that held the turn now reads end-of-file,
                // answers, and lets go of the lock.
                settled(10);
                let _ = std::fs::remove_file(core::stop::flag_for(pid));
                let fresh = match Cli::start(&exe) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = respond(&mut s, "500 Internal Server Error", "text/plain", &format!("stopped, but the cli would not restart: {e}"));
                        return;
                    }
                };
                let new_pid = fresh.child.id();
                if let Ok(mut c) = cli.lock() {
                    *c = fresh;
                }
                child_pid.store(new_pid, Ordering::SeqCst);
                if let Ok(mut l) = own_log_cell.lock() {
                    *l = Some(core::live::log_of(new_pid));
                }
                let _ = respond(&mut s, "200 OK", "application/json", "{\"stopped\":\"restarted\"}");
            }
            ("POST", "/models") => {
                if req.origin.is_none() {
                    let _ = respond(&mut s, "403 Forbidden", "text/plain", "POST /models needs an Origin header");
                    return;
                }
                let body = core::catalog::to_json(&core::catalog::update(true));
                if let Ok(mut c) = catalog.lock() {
                    c.clone_from(&body);
                }
                let _ = respond(&mut s, "200 OK", "application/json", &body);
            }
            ("GET", "/events") => {
                let since = param(&req.path, "since").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
                // Whichever run is currently saying something, which is
                // the one worth watching. A finished run stops touching its
                // file, so an active one wins on mtime without this having
                // to know which processes are alive.
                // The run the page is on, so a poller sticks with it the
                // way the stream does rather than flipping between two.
                let current = param(&req.path, "source").and_then(core::live::named);
                let src = pinned.clone().or_else(|| {
                    core::live::follow(current.as_deref(), own_log.as_deref(), live.busy.load(Ordering::SeqCst))
                });
                let (n, lines, running, name) = match &src {
                    Some(p) => {
                        let (n, lines) = core::live::read_from(p, since);
                        let name = p.file_name().and_then(|f| f.to_str()).unwrap_or("").to_string();
                        (n, lines, running_in(p, own_log.as_deref(), &live), name)
                    }
                    None => (0, Vec::new(), false, String::new()),
                };
                let body = format!(
                    "{{\"n\":{},\"busy\":{},\"running\":{},\"console\":{},\"page\":\"{}\",\"source\":\"{}\",\"lines\":[{}]}}",
                    n,
                    live.busy.load(Ordering::SeqCst),
                    running,
                    std::process::id(),
                    page_version(),
                    json_escape(&name),
                    lines
                        .iter()
                        .map(|l| format!("\"{}\"", json_escape(l)))
                        .collect::<Vec<_>>()
                        .join(",")
                );
                let _ = respond(&mut s, "200 OK", "application/json", &body);
            }
            _ => {
                let _ = respond(&mut s, "404 Not Found", "text/plain", "no such path");
            }
        }
        });
    }
}

/// Ask the providers for their lists at start, and again whenever the
/// cached one is older than `catalog::FRESH_SECS`, for as long as the
/// console runs. A console left open for a week offers this week's models.
fn keep_catalog_fresh(catalog: Arc<Mutex<String>>) {
    std::thread::spawn(move || {
        loop {
            // `update` only fetches what is stale, so waking every minute
            // costs a file read, and a failed fetch is retried a minute
            // later rather than a quarter of an hour.
            let body = core::catalog::to_json(&core::catalog::update(false));
            if let Ok(mut c) = catalog.lock() {
                *c = body;
            }
            std::thread::sleep(Duration::from_secs(60));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(method: &str, origin: Option<&str>) -> Req {
        Req {
            method: method.into(),
            path: "/cmd".into(),
            origin: origin.map(str::to_string),
            last_event_id: None,
            host: Some("127.0.0.1:7777".into()),
            body: "hands".into(),
            too_big: false,
        }
    }

    #[test]
    fn only_a_request_addressed_to_this_machine_is_answered() {
        // DNS rebinding: a page whose domain now resolves to 127.0.0.1 is
        // the console's own origin to the browser, and a GET carries no
        // Origin. Its Host is still its own name.
        let with = |h: Option<&str>| Req { host: h.map(str::to_string), ..req("GET", None) };
        assert!(host_ok(7777, &with(Some("127.0.0.1:7777"))));
        assert!(host_ok(7777, &with(Some("LOCALHOST:7777"))));
        assert!(!host_ok(7777, &with(Some("evil.example:7777"))));
        assert!(!host_ok(7777, &with(Some("127.0.0.1:7788"))));
        assert!(!host_ok(7777, &with(None)));
    }

    #[test]
    fn a_body_over_the_cap_is_refused_rather_than_cut() {
        use std::io::Write as _;
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let at = l.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let mut c = TcpStream::connect(at).unwrap();
            let _ = write!(c, "POST /cmd HTTP/1.1\r\nHost: {at}\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        });
        let (s, _) = l.accept().unwrap();
        t.join().unwrap();
        let r = read_request(&s).unwrap();
        assert!(r.too_big && r.body.is_empty());
    }

    #[test]
    fn a_refused_upload_still_gets_its_refusal() {
        use std::io::{Read as _, Write as _};
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let at = l.local_addr().unwrap();
        // A browser sends the whole body and only then reads the answer. If
        // the server closes with that body unread, Windows resets the
        // connection and the answer is lost.
        let client = std::thread::spawn(move || {
            let mut c = TcpStream::connect(at).unwrap();
            // A server that stops reading must fail this test, not hang it.
            c.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let size = MAX_BODY * 4;
            write!(c, "POST /cmd HTTP/1.1\r\nHost: {at}\r\nContent-Length: {size}\r\n\r\n").unwrap();
            let chunk = vec![b'x'; 8192];
            let mut sent = 0;
            while sent < size {
                // The server may stop reading once it has refused; a failed
                // write is not the thing under test, the answer is.
                if c.write_all(&chunk).is_err() {
                    break;
                }
                sent += chunk.len();
            }
            let mut answer = String::new();
            c.read_to_string(&mut answer).map(|_| answer)
        });
        let (mut s, _) = l.accept().unwrap();
        let r = read_request(&s).unwrap();
        assert!(r.too_big);
        refuse_too_big(&mut s);
        let answer = client.join().unwrap().expect("the refusal must arrive, not a reset");
        assert!(answer.starts_with("HTTP/1.1 413"), "{answer}");
        assert!(answer.contains("64 KB"), "{answer}");
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
    fn only_a_running_turn_lets_a_read_skip_the_queue() {
        assert!(is_turn("say open the workbook"));
        assert!(is_turn("do tidy the sheet"));
        assert!(is_turn("approve"));
        assert!(is_turn("deny not now"));
        // Commands that start with a turn's name are not turns.
        assert!(!is_turn("sayings"));
        assert!(!is_turn("slots"));
        assert!(!is_turn("model opencode-go muse-spark-1.3"));

        let live = Live::default();
        let seen = Seen::default();
        seen.put("slots", "RECEIPT slots pick=auto");
        // No turn: every read goes to the child, never to the copy. A quick
        // command in flight is no reason to answer from what it said before.
        assert_eq!(busy_read(&live, &seen, "slots"), None);
        live.turn.store(true, Ordering::SeqCst);
        assert_eq!(busy_read(&live, &seen, "slots").as_deref(), Some("RECEIPT slots pick=auto"));
        // Only reads, and only what was actually asked before.
        assert_eq!(busy_read(&live, &seen, "model auto"), None);
        assert_eq!(busy_read(&live, &seen, "keys"), None);
    }

    #[test]
    fn a_query_parameter_is_found_by_its_whole_name() {
        let p = "/stream?since=12&source=345.log";
        assert_eq!(param(p, "since"), Some("12"));
        assert_eq!(param(p, "source"), Some("345.log"));
        assert_eq!(param("/events?xsince=9&since=3", "since"), Some("3"));
        assert_eq!(param("/stream", "since"), None);
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
        assert!(!PAGE.contains("__AGENT_TOKEN__"));
        assert!(!PAGE.contains("X-the agent-Token"));
    }

    #[test]
    fn the_page_never_touches_a_binding_before_it_is_declared() {
        // `let` does not hoist. A bootstrap line placed above its own
        // declarations threw a ReferenceError while the script was still
        // evaluating, which stops every line after it: the console came up
        // completely blank, not merely without a progress strip. Cheap to
        // check, and the failure it catches is total.
        for (decl, use_) in [("let tailAt", "tailTimer = setInterval"), ("let tailSrc", "tailSrc =")] {
            let d = PAGE.find(decl).unwrap_or_else(|| panic!("{decl} is gone from the page"));
            let u = PAGE.rfind(use_).unwrap_or_else(|| panic!("{use_} is gone from the page"));
            assert!(d < u, "{use_:?} runs before {decl:?} is initialised, which blanks the page");
        }
    }
}
