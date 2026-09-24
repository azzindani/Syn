//! cli: scriptable REPL over the Runner. Every line is one command;
//! every mutation prints a RECEIPT line. Pipe a script file for E2E tests.
//!
//! Commands:
//!   session <id>            handshake (default session `cli`)
//!   attach excel <f> <sh>   | attach word <f> | attach ppt <f>
//!   read <handle> <sel>     | write <h> <sel> <r1c1,r1c2;r2c1> | para <h> <text>
//!   slide <h> <title> <b1|b2> | xfer <src> <sel> <dst> <title>
//!   undo <h> | events | registry | route <skim|routine|code|deep|vision>
//!   pause | resume | pump | kill | allow <app...> | send <task> <prompt...> (provider POST, needs key)
//!   config                  print resolved models + endpoint (never the key)
//!   hand <pipe> [app...]    connect a hand; names the apps it claims
//!   cdp <host:port> [app..] connect a Chrome DevTools hand (Electron too)
//!   page <app> <match> [u]  register a page/window as a handle (alias: win)
//!   invoke <h> <sel> [act]  press a control on a live handle
//!   hands                   attached hands, in routing order
//!   mark <token>            echo a sentinel (used by the web console)
//!   say <text>              one conversational turn (keeps the history)
//!   chat new|list|open <id>|del <id>|msgs   saved conversations
//!   live <handle>           route that handle's ops to the open document
//!   lread <h> <sel>         queued read of a live handle
//!                           (once `live`, plain `write` also hits the document)
//!   shellallow <p...>       programs the shell hand may run (empty = none)
//!   task <class>            router slot `do` uses (skim|routine|code|deep|vision)
//!   models [refresh] [text] the providers' model catalogs (cached, 15 min)
//!   model <provider> <id>   talk to that model from now on | model auto
//!   think low|medium|high   how hard it thinks | think auto (the slot's own)
//!   do <goal>               run the agent loop until it answers or stops
//!   approve | deny [why]    answer a held shell confirmation
//!   quit

use core::agent::{Agent, Brain, CurlBrain, Step};
use core::bus::OpenFile;
use core::config;
use core::cdp::Cdp;
use core::chats;
use core::hand::Hand;
use core::ops::{Call, ExportArgs, FormatArgs, ReadArgs, StructArgs, WriteArgs};
use core::protocol::new_handle;
use core::provider;
use core::router::{TaskKind, route};
use core::runner::Runner;
use core::shell::ShellPolicy;
use core::Relay;
use std::io::BufRead;

/// Print, and mirror to this run's live log so a console in another
/// process can show the run happening. Every line the CLI emits goes
/// through here: a progress view that shows some of the output is worse
/// than one that shows none, because it looks complete.
macro_rules! pr {
    () => {{ println!(); core::live::append(""); }};
    ($($arg:tt)*) => {{
        let line = format!($($arg)*);
        println!("{line}");
        core::live::append(&line);
    }};
}


/// Submit one op and run it.
///
/// Every op arm goes through here. Calling `ops::execute` from a command
/// would skip the kill switch, the app allowlist, the doom-loop gate, the
/// event feed AND the live-hand routing, so a handle marked live would
/// quietly edit the in-memory model instead of the open document. The agent
/// loop already holds this invariant; the REPL now holds it too.
fn run_op(runner: &mut Runner, relay: &mut Relay, handle: &str, what: &str, call: Call) {
    let id = format!("cli-{what}");
    match runner.run(relay, handle, &id, call) {
        Ok(Some(o)) => pr!("RECEIPT {id} {what} {o:?}"),
        Ok(None) => pr!("ERROR {id} queue not running"),
        Err(e) => pr!("ERROR {id} {e}"),
    }
}



/// Drive the loop until it finishes, stops, or stops for a human.
/// The other model slots to try, in cost order, skipping the one that just
/// failed and any slot configured to the same model id (duplicates would
/// retry the rate-limited model and report a second identical failure).
fn fallbacks(failed: &str) -> Vec<(core::router::Model, String)> {
    use core::router::Model;
    let mut seen = vec![failed.to_string()];
    let mut out = Vec::new();
    for m in Model::ALL {
        let id = config::model_id(m);
        // Two slots may resolve to the same id; retrying it would report the
        // same failure twice and burn a second request saying nothing new.
        if id.is_empty() || seen.contains(&id) {
            continue;
        }
        seen.push(id.clone());
        out.push((m, id));
    }
    out
}

/// What the next turn talks to: the model, its route, and the endpoint.
///
/// The console's picker names a model from a provider's catalog; the slots
/// in `.env` are what is used when it has not. The thinking level is the
/// human's either way, and a fallback keeps it: someone who asked for
/// "low" to save money has not asked for "high" because a model was busy.
struct Choice {
    model: String,
    route: core::router::Route,
    base_url: String,
    key_env: String,
}

fn choose(task: TaskKind, pick: &Option<(core::catalog::Provider, String)>, think: Option<core::router::Effort>) -> Choice {
    let mut r = route(task);
    if let Some(e) = think {
        r.effort = e;
    }
    match pick {
        Some((p, id)) => Choice { model: id.clone(), route: r, base_url: p.base_url.clone(), key_env: p.key_env.clone() },
        None => {
            let (base_url, key_env) = config::endpoint(r.model);
            Choice { model: config::model_id(r.model), route: r, base_url, key_env }
        }
    }
}

/// Persist the conversation after every turn.
///
/// A chat that is only written on a clean exit loses the run that crashed,
/// which is the one the human most wants to look at afterwards.
fn save_chat(id: &str, a: &Agent) {
    let msgs = a.transcript().to_vec();
    let chat = chats::Chat {
        meta: chats::ChatMeta {
            id: id.to_string(),
            title: chats::title_from(&msgs),
            updated: chats::now(),
            turns: 0,
        },
        msgs,
    };
    if let Err(e) = chats::save(&chat) {
        pr!("ERROR chat save {e}");
    }
}

/// Run the loop to its next stopping point and report why it stopped, so a
/// What the run did, in one English sentence, once it is over.
///
/// A transcript already answers this and nobody reads one. `DID Read 12
/// ranges, wrote into 2 documents, and drew 3 charts.` is the line that
/// tells a human whether to look closer.
fn report(a: &Agent) {
    let did = a.summary();
    if !did.is_empty() {
        pr!("DID {did}");
        pr!("RECEIPT did {{\"text\":{did:?}}}");
    }
}

/// caller can decide whether the failure is worth another model.
fn drive(
    a: &mut Agent,
    brain: &mut dyn Brain,
    relay: &mut core::Relay,
    runner: &mut Runner,
    sp: &ShellPolicy,
) -> Option<String> {
    loop {
        let outcome = a.step(brain, relay, runner, sp);
        // What the model is being told about its own budget, told to the
        // human too. The console shows it as a meter beside the composer,
        // and it says what will happen at the end rather than only where
        // the run is now.
        pr!("RECEIPT budget {{\"step\":{},\"max\":{}}}", a.steps(), a.max_steps);
        // Two lines, on purpose. `STEP`/`REFUSED` are the sentence a person
        // reads in the terminal; `RECEIPT step` is the same event as data,
        // and it is the only thing the console parses. Scraping the prose
        // line is what broke the live view the moment its wording changed,
        // and a UI that breaks when a message is reworded is a UI nobody
        // can safely improve.
        let receipt = |a: &Agent, status: core::labels::Status, detail: &str| {
            let (name, args) = a.calls().last()?;
            Some(format!(
                "RECEIPT step {{\"label\":{:?},\"tool\":{:?},\"app\":{:?},\"status\":{:?},\"detail\":{:?}}}",
                core::labels::sentence(name, args, status),
                name,
                core::labels::app(args).unwrap_or_default(),
                status_name(status),
                detail
            ))
        };
        match outcome {
            Step::Ran { tool, detail } => {
                match receipt(a, core::labels::Status::Done, &detail) {
                    Some(r) => {
                        let label = a
                            .calls()
                            .last()
                            .map(|(n, args)| core::labels::sentence(n, args, core::labels::Status::Done))
                            .unwrap_or_else(|| tool.clone());
                        pr!("STEP {label}");
                        pr!("{r}");
                    }
                    None => pr!("STEP {tool}: {detail}"),
                }
            }
            Step::Refused(why) => {
                pr!("REFUSED {why}");
                if let Some(r) = receipt(a, core::labels::Status::Refused, &why) {
                    pr!("{r}");
                }
            }
            Step::Answered(text) => {
                pr!("ANSWER {}", text.replace('\n', " "));
                report(a);
                return None;
            }
            Step::Stopped(why) => {
                pr!("STOPPED {why}");
                report(a);
                return Some(why);
            }
            Step::NeedsApproval(p) => {
                pr!("CONFIRM {}", p.preview);
                pr!("        reason given: {}", p.why);
                pr!("        respond with `approve` or `deny <reason>`");
                return None;
            }
        }
    }
}

/// The wire name for a status, shared by the live receipts and the
/// `LABEL` lines a reopened conversation is rebuilt from.
fn status_name(s: core::labels::Status) -> &'static str {
    match s {
        core::labels::Status::Running => "running",
        core::labels::Status::Done => "done",
        core::labels::Status::Failed => "failed",
        core::labels::Status::Refused => "refused",
        core::labels::Status::Stopped => "stopped",
    }
}


/// Ask the same model again, waiting the way opencode waits.
///
/// Ported from `session/retry.ts`; see `docs/DIGEST-06-opencode-loop.md`
/// §3. Three things this does that the fixed `[2s, 6s, 15s]` before it did
/// not:
///
/// - **five attempts, not three**, which is opencode's `RETRY_MAX_RETRIES`;
/// - **exponential with jitter** rather than a fixed ladder, so a fleet of
///   clients rate-limited together do not all return in the same instant;
/// - and the delay is capped at 30s, their `RETRY_MAX_DELAY_NO_HEADERS`.
///
/// What it still cannot do is honour `Retry-After`, because `send_via_curl`
/// throws the response headers away. `provider::retry_after_ms` is written
/// and tested and has no caller yet — capturing headers means `curl -D` and
/// a second output stream to parse, which is a change to the transport
/// rather than to the retry policy. Noted here so the gap is visible at
/// the place it matters instead of only in the digest.
fn wait_and_retry(
    model: &str,
    mut stopped: Option<String>,
    a: &mut Agent,
    brain: &mut dyn Brain,
    relay: &mut core::Relay,
    runner: &mut Runner,
    sp: &ShellPolicy,
) -> Option<String> {
    for attempt in 1..=provider::RETRY_MAX_RETRIES {
        let Some(why) = stopped.as_deref() else { break };
        if !provider::worth_waiting(why) {
            break;
        }
        // Jitter needs a number that differs per attempt and per process.
        // `core` has no rng and will not grow one for this: the clock's
        // sub-second remainder is arbitrary enough to stop a fleet
        // synchronising, which is all the jitter is for.
        let pct = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_millis() as u64)
            .unwrap_or(0);
        let ms = provider::retry_delay_ms(attempt, pct);
        pr!("RECEIPT waiting {ms}ms (attempt {attempt}/{}) then asking {model} again", provider::RETRY_MAX_RETRIES);
        std::thread::sleep(std::time::Duration::from_millis(ms));
        stopped = drive(a, brain, relay, runner, sp);
    }
    stopped
}

/// A Word style name as one CLI token: underscores stand in for spaces, and
/// a bare `.` means body text. Keeps the prose last on the line.
fn word_style(tok: &str) -> String {
    if tok == "." { String::new() } else { tok.replace('_', " ") }
}

fn main() {
    core::live::begin();
    // Deployment config before anything else: .env seeds the process env,
    // a real shell export always wins. Key names only are printed.
    if let Some(l) = config::load_env(&std::env::current_dir().unwrap_or_default()) {
        pr!("RECEIPT env file={} applied={} kept={}", l.path.display(), l.applied.join(","), l.skipped.join(","));
    }
    let mut relay = Relay::new();
    let mut session = "cli".to_string();
    let mut runner = Runner::new(&session);
    relay.handshake(&session, "cli");

    let stdin = std::io::stdin();
    let mut buf: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let mut journal_path: Option<String> = None;
    // The agent loop and the one hand that leaves the documents behind.
    // The allowlist starts empty: nothing may run until it is named.
    let mut agent: Option<Agent> = None;
    // The conversation `say` appends to. One per CLI process until `chat
    // open` switches it; saved after every turn so history survives a crash
    // rather than only a clean exit.
    let mut chat_id: String = chats::new_id();
    let mut shell_policy = ShellPolicy::default();
    // Which router slot `do` runs on. Switchable because free-tier models
    // rate-limit independently: a 429 on one slot is not a reason to stop.
    let mut task = TaskKind::Routine;
    // A model picked from a catalog, and a thinking level, when the human
    // has chosen them. None means the slot decides.
    let mut pick: Option<(core::catalog::Provider, String)> = None;
    let mut think: Option<core::router::Effort> = None;
    // Where the current run is being sent, so an approval resumes it on
    // the same endpoint: after a fallback, or on a picked provider, the
    // slot's own endpoint is somewhere else.
    let mut on: (String, String) = config::endpoint(route(task).model);
    // Every live line since process start: enabling the journal backfills
    // this so a replay is always self-contained (replay/session lines kept,
    // nested replay lines skipped).
    let mut history: Vec<String> = Vec::new();
    loop {
        let (line, live) = match buf.pop_front() {
            Some(l) => (l, false),
            None => match stdin.lock().lines().next() {
                // Strip a leading BOM. Windows producers add one freely --
                // PowerShell's $OutputEncoding does it when piping to a
                // native exe -- and it has now cost this project three
                // debugging sessions, two of them on the sidecar pipe.
                // Tolerating it here is one line; diagnosing it again is not.
                Some(Ok(l)) => (l.strip_prefix('\u{feff}').unwrap_or(&l).to_string(), true),
                _ => break,
            },
        };
        // Journal live input only: a `replay <path>` line re-expands on recovery.
        if live {
            use std::io::Write;
            if !line.starts_with("replay ") {
                history.push(line.clone());
            }
            if let Some(p) = &journal_path
                && let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p)
            {
                let _ = writeln!(f, "{line}");
            }
        }
        let parts: Vec<&str> = line.splitn(2, ' ').collect();
        if parts[0].is_empty() {
            continue;
        }
        let (cmd, rest) = (parts[0], parts.get(1).unwrap_or(&""));
        match cmd {
            "" | "#" => {}
            "quit" | "exit" => break,
            // Not a no-op: the console frames one run of CLI output by
            // sending `mark <nonce>` and reading until the echo comes back.
            // Marking a handle live is `live`.
            "mark" => pr!("RECEIPT mark={}", rest.trim()),
            "session" => {
                session = rest.to_string();
                relay.handshake(&session, "cli");
                runner = Runner::new(&session);
                pr!("RECEIPT session={session}");
            }
            "attach" => {
                let a: Vec<&str> = rest.split_whitespace().collect();
                // A handle is app:file:unit split on ':', so a Windows drive
                // letter mints one with four parts that every reader then
                // mis-splits. It attached happily and failed on the first op.
                // The file component is a name here, not a path: the hand
                // finds the open document by it.
                if let Some(bad) = a.iter().skip(1).find(|p| p.contains(':')) {
                    pr!(
                        "ERROR attach {bad:?} contains ':', which is what separates a handle: name the open document, not its path"
                    );
                    continue;
                }
                let (file, kind) = match a.as_slice() {
                    ["excel", f, unit] => (OpenFile::blank_excel(), new_handle("excel", f, unit)),
                    ["word", f] => (OpenFile::blank_word(), new_handle("word", f, "body")),
                    ["ppt", f] => (OpenFile::blank_ppt(), new_handle("ppt", f, "deck")),
                    _ => {
                        pr!("ERROR usage: attach excel <file> <sheet> | attach word <file> | attach ppt <file>");
                        continue;
                    }
                };
                relay.attach(&session, kind.clone(), file);
                pr!("RECEIPT attached={kind}");
            }
            // A sentinel so a non-interactive driver knows where one
            // command's output ends. Commands print a variable number of
            // lines, so a reader with no marker either guesses or blocks.
            // What this deployment actually has, so nothing downstream has
            // to hardcode a model name, a pipe or a port.
            "slots" => {
                for m in core::router::Model::ALL {
                    let r = core::router::route_of(m);
                    pr!(
                        "SLOT {{\"slot\":\"{m:?}\",\"task\":\"{}\",\"model\":\"{}\",\"rank\":{}}}",
                        core::router::task_name(core::router::task_of(m)),
                        config::model_id(m),
                        r.cost_rank
                    );
                }
                // And what the picker last chose, so a second window, or a
                // reload in a fresh browser, shows what the next turn will
                // really use rather than what that browser remembers.
                let chosen = match &pick {
                    Some((p, id)) => format!("pick={id} provider={}", p.name),
                    None => "pick=auto".to_string(),
                };
                let level = think.map(provider::effort_str).unwrap_or("auto");
                pr!("RECEIPT slots current={} {chosen} think={level}", core::router::task_name(task));
            }
            "wiring" => {
                for app in ["excel", "word", "ppt", "uia"] {
                    if let Some(p) = config::pipe_for(app) {
                        pr!("WIRE {{\"kind\":\"pipe\",\"app\":\"{app}\",\"at\":\"{p}\"}}");
                    }
                }
                if let Some(a) = config::cdp_addr() {
                    pr!("WIRE {{\"kind\":\"cdp\",\"app\":\"web\",\"at\":\"{a}\"}}");
                }
                pr!("RECEIPT wiring");
            }
            "registry" => pr!("RECEIPT registry={:?}", relay.registry(&session).unwrap_or_default()),
            "read" => {
                let a: Vec<&str> = rest.splitn(2, ' ').collect();
                if a.len() < 2 {
                    pr!("ERROR usage: read <handle> <selector>");
                    continue;
                }
                run_op(&mut runner, &mut relay, a[0], "read", Call::Read(ReadArgs { selector: a[1].into() }));
            }
            "write" => {
                let a: Vec<&str> = rest.splitn(3, ' ').collect();
                if a.len() < 3 {
                    pr!("ERROR usage: write <handle> <selector> <v11,v12;r21>");
                    continue;
                }
                let call = Call::Write(WriteArgs { selector: a[1].into(), values: core::tools::grid(a[2]) });
                run_op(&mut runner, &mut relay, a[0], "write", call);
            }
            // The style is one token so the prose can keep its spaces and
            // stay last: "Heading_1" for "Heading 1", "." for body text.
            "para" => {
                let a: Vec<&str> = rest.splitn(3, ' ').collect();
                if a.len() < 3 {
                    pr!("ERROR usage: para <handle> <style|.> <text>   (style: Heading_1, Title, Quote)");
                    continue;
                }
                let call = Call::Struct(StructArgs::InsertParagraph {
                    text: a[2].into(),
                    style: word_style(a[1]),
                    at: String::new(),
                });
                run_op(&mut runner, &mut relay, a[0], "para", call);
            }
            "wtable" => {
                let a: Vec<&str> = rest.splitn(3, ' ').collect();
                if a.len() < 3 {
                    pr!("ERROR usage: wtable <handle> <style|.> <cells by | rows by ;>");
                    continue;
                }
                let call = Call::Struct(StructArgs::InsertTable {
                    rows: core::tools::grid(a[2]),
                    style: word_style(a[1]),
                    selector: String::new(),
                    at: String::new(),
                });
                run_op(&mut runner, &mut relay, a[0], "wtable", call);
            }
            "pagebreak" => {
                let a: Vec<&str> = rest.splitn(2, ' ').collect();
                if a.is_empty() || a[0].is_empty() {
                    pr!("ERROR usage: pagebreak <handle> [page|section]");
                    continue;
                }
                let call = Call::Struct(StructArgs::PageBreak {
                    kind: a.get(1).unwrap_or(&"").trim().to_string(),
                });
                run_op(&mut runner, &mut relay, a[0], "pagebreak", call);
            }
            "contents" => {
                let a: Vec<&str> = rest.splitn(2, ' ').collect();
                if a.is_empty() || a[0].is_empty() {
                    pr!("ERROR usage: contents <handle> [heading]");
                    continue;
                }
                let call = Call::Struct(StructArgs::Contents {
                    title: a.get(1).unwrap_or(&"").trim().to_string(),
                });
                run_op(&mut runner, &mut relay, a[0], "contents", call);
            }
            "pagenumbers" => {
                let a: Vec<&str> = rest.splitn(2, ' ').collect();
                if a.is_empty() || a[0].is_empty() {
                    pr!("ERROR usage: pagenumbers <handle> [footer text]");
                    continue;
                }
                let call = Call::Struct(StructArgs::PageNumbers {
                    text: a.get(1).unwrap_or(&"").trim().to_string(),
                });
                run_op(&mut runner, &mut relay, a[0], "pagenumbers", call);
            }
            "picture" => {
                let a: Vec<&str> = rest.splitn(3, ' ').collect();
                if a.len() < 2 {
                    pr!("ERROR usage: picture <handle> <path> [width in points]");
                    continue;
                }
                let call = Call::Struct(StructArgs::Picture {
                    path: a[1].into(),
                    width: a.get(2).unwrap_or(&"").trim().to_string(),
                    selector: String::new(),
                });
                run_op(&mut runner, &mut relay, a[0], "picture", call);
            }
            // Layout first, title last: the title has spaces in it, the
            // same reason `para` and `chart` order their arguments that way.
            // Title and bullets are both free text, so a space cannot
            // separate them. The title runs to the first `|`, and the
            // bullets are what follows -- the same `|` the tool uses.
            "slide" => {
                let a: Vec<&str> = rest.splitn(3, ' ').collect();
                if a.len() < 3 {
                    pr!(
                        "ERROR usage: slide <handle> <layout|.> <title>[|b1|b2]   \
                         (layout: title, titleContent, sectionHeader, twoContent, titleOnly, blank)"
                    );
                    continue;
                }
                let (title, bullets) = match a[2].split_once('|') {
                    Some((t, b)) => (t.trim().to_string(), core::tools::grid(b).into_iter().next().unwrap_or_default()),
                    None => (a[2].trim().to_string(), Vec::new()),
                };
                let call = Call::Struct(StructArgs::CreateSlide {
                    title,
                    bullets,
                    layout: if a[1] == "." { String::new() } else { a[1].into() },
                });
                run_op(&mut runner, &mut relay, a[0], "slide", call);
            }
            // Geometry is one token and comes before the free text, the
            // same order `para` and `chart` settled on.
            "sfigure" => {
                let a: Vec<&str> = rest.splitn(4, ' ').collect();
                if a.len() < 4 {
                    pr!("ERROR usage: sfigure <handle> <s3> <left,top,width,height|.> <path>");
                    continue;
                }
                let call = Call::Struct(StructArgs::Picture {
                    path: a[3].trim().into(),
                    width: if a[2] == "." { String::new() } else { a[2].into() },
                    selector: a[1].into(),
                });
                run_op(&mut runner, &mut relay, a[0], "sfigure", call);
            }
            "stable" => {
                let a: Vec<&str> = rest.splitn(4, ' ').collect();
                if a.len() < 4 {
                    pr!("ERROR usage: stable <handle> <s3> <left,top,width,height|.> <cells by | rows by ;>");
                    continue;
                }
                let call = Call::Struct(StructArgs::InsertTable {
                    rows: core::tools::grid(a[3].trim()),
                    style: if a[2] == "." { String::new() } else { a[2].into() },
                    selector: a[1].into(),
                    at: String::new(),
                });
                run_op(&mut runner, &mut relay, a[0], "stable", call);
            }
            "xfer" => {
                let a: Vec<&str> = rest.splitn(4, ' ').collect();
                if a.len() < 4 {
                    pr!("ERROR usage: xfer <src> <selector> <dst> <title>");
                    continue;
                }
                let call = Call::Struct(StructArgs::Transfer { from: a[0].into(), selector: a[1].into(), title: a[3].into() });
                run_op(&mut runner, &mut relay, a[2], "xfer", call);
            }
            "sheet" => {
                let a: Vec<&str> = rest.splitn(2, ' ').collect();
                if a.len() < 2 {
                    pr!("ERROR usage: sheet <handle> <name>");
                    continue;
                }
                let call = Call::Struct(StructArgs::AddSheet { name: a[1].trim().into() });
                run_op(&mut runner, &mut relay, a[0], "sheet", call);
            }
            "pivot" => {
                let a: Vec<&str> = rest.split_whitespace().collect();
                // cols is optional: a pivot down one axis is still a pivot.
                let (h, src, rows, values, at, cols) = match a.as_slice() {
                    [h, src, rows, values, at] => (*h, *src, *rows, *values, *at, ""),
                    [h, src, rows, values, at, cols] => (*h, *src, *rows, *values, *at, *cols),
                    _ => {
                        pr!("ERROR usage: pivot <handle> <source> <rowField> <valueField> <at> [colField]");
                        continue;
                    }
                };
                let call = Call::Struct(StructArgs::Pivot {
                    source: src.into(),
                    rows: rows.into(),
                    cols: cols.into(),
                    values: values.into(),
                    at: at.into(),
                });
                run_op(&mut runner, &mut relay, h, "pivot", call);
            }
            "table" => {
                let a: Vec<&str> = rest.split_whitespace().collect();
                let [h, src, name] = a.as_slice() else {
                    pr!("ERROR usage: table <handle> <source> <name>");
                    continue;
                };
                let call = Call::Struct(StructArgs::Table { source: (*src).into(), name: (*name).into() });
                run_op(&mut runner, &mut relay, h, "table", call);
            }
            "name" => {
                let a: Vec<&str> = rest.split_whitespace().collect();
                let [h, name, at] = a.as_slice() else {
                    pr!("ERROR usage: name <handle> <name> <target>");
                    continue;
                };
                let call = Call::Struct(StructArgs::Name { name: (*name).into(), at: (*at).into() });
                run_op(&mut runner, &mut relay, h, "name", call);
            }
            "macro" => {
                // `macro <handle> <write|run|read|list> <module> [code...]`
                // Code runs to the end of the line, so the short fields
                // come first -- the same rule the geometry arguments
                // follow, and for the same reason.
                let mut p = rest.splitn(4, char::is_whitespace);
                let (Some(h), Some(action)) = (p.next(), p.next()) else {
                    pr!("ERROR usage: macro <handle> <write|run|read|list> [module] [code]");
                    continue;
                };
                let module = p.next().unwrap_or("SynMacros").to_string();
                let tail = p.next().unwrap_or("").to_string();
                // `run` names the macro; `write` carries the source. Both
                // ride in the last field, so which one it is depends on
                // the action rather than on the position.
                let (code, name) = match action {
                    "run" => (String::new(), if tail.is_empty() { module.clone() } else { tail }),
                    _ => (tail.replace("\\n", "\n"), String::new()),
                };
                let call = Call::Struct(StructArgs::Macro {
                    action: action.into(),
                    module,
                    code,
                    name,
                });
                run_op(&mut runner, &mut relay, h, "macro", call);
            }
            "conditional" => {
                let a: Vec<&str> = rest.split_whitespace().collect();
                let [h, sel, rule] = a.as_slice() else {
                    pr!("ERROR usage: conditional <handle> <selector> <dataBar|colorScale|iconSet|top10|greaterThan=N>");
                    continue;
                };
                let call = Call::Struct(StructArgs::Conditional { selector: (*sel).into(), rule: (*rule).into() });
                run_op(&mut runner, &mut relay, h, "conditional", call);
            }
            "slicer" => {
                let a: Vec<&str> = rest.split_whitespace().collect();
                let (h, field, at, pivot) = match a.as_slice() {
                    [h, field, at] => (*h, *field, *at, ""),
                    [h, field, at, pivot] => (*h, *field, *at, *pivot),
                    _ => {
                        pr!("ERROR usage: slicer <handle> <field> <at> [pivotName]");
                        continue;
                    }
                };
                let call = Call::Struct(StructArgs::Slicer {
                    pivot: pivot.into(),
                    field: field.into(),
                    at: at.into(),
                });
                run_op(&mut runner, &mut relay, h, "slicer", call);
            }
            // Style before title, for the same reason `para` puts it before
            // the prose: the title has spaces in it and has to come last.
            "chart" => {
                let a: Vec<&str> = rest.splitn(6, ' ').collect();
                if a.len() < 4 {
                    pr!(
                        "ERROR usage: chart <handle> <line|bar|column|pie> <source> <at> [style|.] [title]"
                    );
                    continue;
                }
                let style = a.get(4).copied().unwrap_or(".");
                let call = Call::Struct(StructArgs::Chart {
                    kind: a[1].into(),
                    source: a[2].into(),
                    at: a[3].into(),
                    title: a.get(5).unwrap_or(&"").trim().to_string(),
                    style: if style == "." { String::new() } else { style.to_string() },
                });
                run_op(&mut runner, &mut relay, a[0], "chart", call);
            }
            "undo" => run_op(&mut runner, &mut relay, rest.trim(), "undo", Call::Undo),
            "export" => {
                let a: Vec<&str> = rest.splitn(3, ' ').collect();
                let fmt = a.get(1).unwrap_or(&"summary").to_string();
                let path = a.get(2).map(|s| s.to_string());
                let call = Call::Export(ExportArgs { format: fmt, path, sheet: None });
                run_op(&mut runner, &mut relay, a[0], "export", call);
            }
            "format" => {
                let a: Vec<&str> = rest.splitn(3, ' ').collect();
                if a.len() < 3 {
                    pr!("ERROR usage: format <handle> <selector> <k=v;k=v>  e.g. bold=1;numberFormat=#,##0");
                    continue;
                }
                // Semicolon, not comma: a number format is "#,##0" and
                // splitting styles on a comma cuts it in half.
                let mut style = Vec::new();
                for kv in a[2].split(';') {
                    match kv.split_once('=') {
                        Some((k, v)) => style.push((k.to_string(), v.to_string())),
                        None => {
                            pr!("ERROR bad style pair {kv:?}, want k=v");
                            continue;
                        }
                    }
                }
                let call = Call::Format(FormatArgs { selector: a[1].into(), style });
                run_op(&mut runner, &mut relay, a[0], "format", call);
            }
            "events" => {
                for e in relay.events(&session).unwrap_or_default() {
                    pr!("EVENT {} {} {}", e.t, e.handle, e.detail);
                }
                pr!("RECEIPT events-end");
            }
            "route" => {
                let task = match *rest {
                    "skim" => TaskKind::Skim,
                    "routine" => TaskKind::Routine,
                    "code" => TaskKind::Code,
                    "deep" => TaskKind::DeepReasoning,
                    _ => TaskKind::VisionFallback,
                };
                let r = route(task);
                pr!(
                    "RECEIPT route model={:?} id={} effort={:?} rank={}",
                    r.model,
                    config::model_id(r.model),
                    r.effort,
                    r.cost_rank
                );
            }
            "config" => pr!("RECEIPT config {}", config::describe()),
            "task" => {
                // One naming table, in the router. A second copy here is
                // how a command and a picker end up disagreeing about what
                // "code" means.
                task = match core::router::task_named(rest.trim()) {
                    Some(t) => t,
                    None => {
                        pr!("ERROR task: unknown class {:?} (skim|routine|code|deep|vision)", rest.trim());
                        continue;
                    }
                };
                let r = route(task);
                pr!("RECEIPT task={:?} model={}", task, config::model_id(r.model));
            }
            "model" => {
                let a: Vec<&str> = rest.split_whitespace().collect();
                match a.as_slice() {
                    ["auto"] => {
                        pick = None;
                        pr!("RECEIPT model=auto slot={}", config::model_id(route(task).model));
                    }
                    [prov, id] => {
                        // An id goes into a request body and nowhere else,
                        // but it is still capped and checked: it arrives
                        // from a page, and before that from a provider.
                        if id.len() > 200 || id.chars().any(|c| c.is_control() || c == '"' || c == '\\') {
                            pr!("ERROR model: that id is not one a provider would send");
                            continue;
                        }
                        let Some(p) = core::catalog::provider_named(prov) else {
                            let have: Vec<String> = core::catalog::providers().into_iter().map(|p| p.name).collect();
                            pr!("ERROR model: no provider {prov:?} here (have: {})", have.join(", "));
                            continue;
                        };
                        pr!("RECEIPT model={id} provider={}", p.name);
                        pick = Some((p, id.to_string()));
                    }
                    _ => pr!("ERROR usage: model <provider> <id> | model auto"),
                }
            }
            "think" => {
                think = match rest.trim() {
                    "auto" => None,
                    level => match core::router::effort_named(level) {
                        Some(e) => Some(e),
                        None => {
                            pr!("ERROR think: {level:?} is not a level (auto|low|medium|high)");
                            continue;
                        }
                    },
                };
                let e = choose(task, &pick, think).route.effort;
                pr!("RECEIPT think={} effort={}", rest.trim(), provider::effort_str(e));
            }
            "models" => {
                // `models [refresh] [text]`. The console keeps its own copy
                // fresh; this is the same list for someone at a terminal.
                let (force, text) = match rest.trim().strip_prefix("refresh") {
                    Some(t) => (true, t.trim()),
                    None => (false, rest.trim()),
                };
                let needle = text.to_ascii_lowercase();
                for s in core::catalog::update(force) {
                    if let Some(e) = &s.error {
                        pr!("NOTE {}: {e}", s.provider.name);
                    }
                    let hits: Vec<_> = s
                        .entries
                        .iter()
                        .filter(|e| needle.is_empty() || e.id.to_ascii_lowercase().contains(&needle) || e.name.to_ascii_lowercase().contains(&needle))
                        .collect();
                    for e in hits.iter().take(40) {
                        pr!("MODEL {} {}  {}", s.provider.name, e.id, e.name);
                    }
                    pr!("RECEIPT models provider={} count={} shown={}", s.provider.name, s.entries.len(), hits.len().min(40));
                }
            }
            "shellallow" => {
                let progs: Vec<&str> = rest.split_whitespace().collect();
                shell_policy = ShellPolicy::new(&progs);
                pr!("RECEIPT shell-allowlist={:?}", shell_policy.allowed());
            }
            "do" => {
                if rest.trim().is_empty() {
                    pr!("ERROR usage: do <goal>");
                    continue;
                }
                let c = choose(task, &pick, think);
                if core::auth::resolve(&c.base_url, &c.key_env).is_none() {
                    pr!("ERROR do: no key for {}: set {} (see .env.example)", c.base_url, c.key_env);
                    continue;
                }
                let (r, model) = (c.route, c.model);
                // A new goal is the human's go-ahead after a repeated call
                // or a dead pipe froze the last run.
                runner.thaw(&mut relay);
                // The slot's own endpoint and key, not the global pair: a
                // fallback chain whose links all point at one provider
                // shares that provider's bad minute, and is one link.
                on = (c.base_url, c.key_env);
                let mut brain = CurlBrain { base_url: on.0.clone(), api_key_env: on.1.clone() };
                let mut a = Agent::new(&session, rest.trim(), &model, r);
                a.fit_context(core::catalog::context_of(&model));
                pr!("RECEIPT do model={model} max_steps={}", a.max_steps);
                drive(&mut a, &mut brain, &mut relay, &mut runner, &shell_policy);
                agent = Some(a);
            }
            "say" => {
                // One conversational turn. Unlike `do`, this keeps the
                // transcript: a chat where every message starts a new agent
                // is not a chat, it is a series of strangers.
                let text = rest.trim();
                if text.is_empty() {
                    pr!("ERROR usage: say <text>");
                    continue;
                }
                let c = choose(task, &pick, think);
                if core::auth::resolve(&c.base_url, &c.key_env).is_none() {
                    pr!("ERROR say: no key for {}: set {} (see .env.example)", c.base_url, c.key_env);
                    continue;
                }
                let (r, model) = (c.route, c.model);
                // The human's next message is the confirmation the
                // repeated-call gate asks for. Without this, one stopped
                // turn left the runner paused and every message after it
                // died on its first call with "queue is not running", with
                // nothing in the console able to resume it.
                runner.thaw(&mut relay);
                match agent.as_mut() {
                    Some(a) => {
                        if let Err(e) = a.follow_up(text) {
                            pr!("ERROR say {e}");
                            continue;
                        }
                        // Apply the current slot every turn, so switching
                        // after a 429 actually moves the next request.
                        a.retarget(&model, r);
                    }
                    None => agent = Some(Agent::new(&session, text, &model, r)),
                }
                let a = agent.as_mut().expect("just set");
                // Sized to the model this turn goes to, which the picker
                // may have changed since the last one.
                a.fit_context(core::catalog::context_of(&model));
                // Before every turn, not once at construction: a hand
                // attached mid-conversation has to be visible to the next
                // message, or the model keeps saying it cannot reach anything.
                let open: Vec<(String, bool)> = relay
                    .registry(&session)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|h| {
                        let live = runner.is_live(&h);
                        (h, live)
                    })
                    .collect();
                a.show_registry(&open);
                pr!("RECEIPT say model={model} open={}", open.len());
                // The slot's own endpoint and key, not the global pair: a
                // fallback chain whose links all point at one provider
                // shares that provider's bad minute, and is one link.
                on = (c.base_url, c.key_env);
                let mut brain = CurlBrain { base_url: on.0.clone(), api_key_env: on.1.clone() };
                let mut stopped = drive(a, &mut brain, &mut relay, &mut runner, &shell_policy);

                // Wait and ask the SAME model again before giving up on
                // it. A free-tier limit is per minute far more often than
                // per day, and an overloaded provider recovers in
                // seconds. Two capability runs abandoned the model they
                // were meant to be testing at step ~15 on a single blip
                // and spent the rest of the run on a weaker slot, which
                // is a worse outcome than pausing for six seconds.
                stopped = wait_and_retry(&model, stopped, a, &mut brain, &mut relay, &mut runner, &shell_policy);

                // Only once the model has had its chances: free-tier slots
                // rate-limit independently, so a 429 on one says nothing
                // about the next. Walk the rest rather than handing the
                // human a provider's JSON and asking them to know which
                // slot to pick.
                for (m, id) in fallbacks(&model) {
                    let Some(why) = stopped.as_deref() else { break };
                    if !provider::worth_another_model(why) {
                        break;
                    }
                    pr!("RECEIPT retry model={id}");
                    let mut r = core::router::route_of(m);
                    if let Some(e) = think {
                        r.effort = e;
                    }
                    a.retarget(&id, r);
                    a.fit_context(core::catalog::context_of(&id));
                    // Each slot on its own endpoint. This used to keep the
                    // first brain, which sent a slot's id to whichever host
                    // the failed model lived on -- harmless while every slot
                    // was OpenRouter, wrong once a model can be picked from
                    // another provider.
                    on = config::endpoint(m);
                    brain = CurlBrain { base_url: on.0.clone(), api_key_env: on.1.clone() };
                    stopped = drive(a, &mut brain, &mut relay, &mut runner, &shell_policy);
                    // The new slot gets the same patience as the first.
                    stopped = wait_and_retry(&id, stopped, a, &mut brain, &mut relay, &mut runner, &shell_policy);
                }
                if let Some(why) = &stopped
                    && provider::worth_another_model(why)
                {
                    pr!("STOPPED every model slot is rate limited right now: wait a moment and send again");
                }
                // Into the transcript before saving, or the conversation
                // records the attempt and not why it ended.
                if let Some(why) = &stopped {
                    a.note_stop(why);
                }
                save_chat(&chat_id, a);
            }
            "chat" => {
                let mut a = rest.split_whitespace();
                match a.next().unwrap_or("msgs") {
                    "new" => {
                        chat_id = chats::new_id();
                        agent = None;
                        pr!("RECEIPT chat={chat_id}");
                    }
                    "list" => {
                        for m in chats::list() {
                            pr!("CHAT {}", chats::meta_json(&m));
                        }
                        pr!("RECEIPT chat current={chat_id}");
                    }
                    "open" => {
                        let Some(id) = a.next() else {
                            pr!("ERROR usage: chat open <id>");
                            continue;
                        };
                        match chats::load(id) {
                            Ok(c) => {
                                let r = route(task);
                                agent = Some(Agent::resume(&session, c.msgs, &config::model_id(r.model), r));
                                chat_id = c.meta.id;
                                pr!("RECEIPT chat={chat_id} title={:?}", c.meta.title);
                            }
                            Err(e) => pr!("ERROR chat open {e}"),
                        }
                    }
                    "del" => {
                        let Some(id) = a.next() else {
                            pr!("ERROR usage: chat del <id>");
                            continue;
                        };
                        match chats::delete(id) {
                            Ok(()) => {
                                if id == chat_id {
                                    chat_id = chats::new_id();
                                    agent = None;
                                }
                                pr!("RECEIPT deleted={id}");
                            }
                            Err(e) => pr!("ERROR chat del {e}"),
                        }
                    }
                    "msgs" => {
                        // The transcript as the UI wants it, plus whatever
                        // the run is currently waiting on, so the page never
                        // has to infer state from receipt text.
                        if let Some(ag) = agent.as_ref() {
                            for m in ag.transcript() {
                                pr!("MSG {}", chats::msg_json(m));
                            }
                            // One English sentence per call, built by the
                            // same table the feed and the CLI use. The page
                            // must not re-derive these: a second
                            // implementation in JavaScript is a second
                            // thing to keep in step with the surface, and
                            // it would not be covered by the tests that
                            // stop a new verb reaching a human as raw JSON.
                            for (id, name, args, status) in core::chats::call_labels(ag.transcript()) {
                                pr!(
                                    "LABEL {{\"id\":{:?},\"text\":{:?},\"app\":{:?},\"status\":{:?}}}",
                                    id,
                                    core::labels::sentence(&name, &args, status),
                                    core::labels::app(&args).unwrap_or_default(),
                                    status_name(status)
                                );
                            }
                            if let Some(p) = ag.pending() {
                                pr!(
                                    "PENDING {{\"program\":{:?},\"preview\":{:?},\"why\":{:?}}}",
                                    p.program, p.preview, p.why
                                );
                            }
                        }
                        pr!("RECEIPT chat={chat_id}");
                    }
                    other => pr!("ERROR chat: unknown {other:?}, want new|list|open|del|msgs"),
                }
            }
            "approve" | "deny" => {
                let Some(a) = agent.as_mut() else {
                    pr!("ERROR {cmd}: no run in progress");
                    continue;
                };
                let outcome = if cmd == "approve" {
                    a.approve(&mut relay, &shell_policy)
                } else {
                    a.deny(&mut relay, rest.trim())
                };
                match &outcome {
                    Step::Ran { tool, detail } => pr!("STEP {tool}: {detail}"),
                    Step::Refused(why) => pr!("REFUSED {why}"),
                    other => pr!("RECEIPT {other:?}"),
                }
                if !matches!(outcome, Step::Stopped(_)) {
                    // The route the run is actually on, which a fallback
                    // may have changed since the REPL's task slot was set.
                    let mut brain = CurlBrain { base_url: on.0.clone(), api_key_env: on.1.clone() };
                    let _ = drive(a, &mut brain, &mut relay, &mut runner, &shell_policy);
                }
                save_chat(&chat_id, a);
            }
            "open" => {
                // `open <app> <path>`: the harness setting up its own
                // session. Everything else here addresses a document that
                // is already open; this is how one gets that way, so a run
                // no longer needs a person to open three files by hand
                // before it can start.
                let mut parts = rest.splitn(2, char::is_whitespace);
                let (Some(app), Some(path)) = (parts.next(), parts.next()) else {
                    pr!("ERROR usage: open <app> <path>");
                    continue;
                };
                match runner.open_file(app.trim(), path.trim()) {
                    Ok(detail) => pr!("RECEIPT open {app} {detail}"),
                    Err(e) => pr!("ERROR open {e}"),
                }
            }
            "hand" => {
                // `hand <pipe> [app...]`: with apps, this hand claims them;
                // with none it is the catch-all for handles no other hand
                // claims. Several hands can be attached at once.
                let mut parts = rest.split_whitespace();
                let Some(pipe) = parts.next() else {
                    pr!("ERROR usage: hand <pipe> [app...]");
                    continue;
                };
                let apps: Vec<String> = parts.map(str::to_string).collect();
                match Hand::connect(pipe) {
                    Ok(h) => {
                        let claims = if apps.is_empty() { "any".to_string() } else { apps.join(",") };
                        runner.attach_hand_as(pipe, apps, Box::new(h));
                        // Reconnecting is how a human recovers from a dead
                        // pipe, which froze the run.
                        runner.thaw(&mut relay);
                        pr!("RECEIPT hand={pipe} claims={claims}");
                    }
                    Err(e) => pr!("ERROR hand {e}"),
                }
            }
            "cdp" => {
                // One hand, every Chromium on the box: browsers and every
                // Electron app started with --remote-debugging-port.
                let mut parts = rest.split_whitespace();
                let Some(addr) = parts.next() else {
                    pr!("ERROR usage: cdp <host:port> [app...]");
                    continue;
                };
                let apps: Vec<String> = parts.map(str::to_string).collect();
                match Cdp::connect(addr) {
                    Ok(c) => {
                        let open: Vec<String> = c
                            .targets()
                            .iter()
                            .filter(|t| t.kind == "page")
                            .map(|t| t.title.clone())
                            .collect();
                        let claims = if apps.is_empty() { "any".to_string() } else { apps.join(",") };
                        let name = format!("cdp-{addr}");
                        runner.attach_hand_as(&name, apps, Box::new(c));
                        runner.thaw(&mut relay);
                        pr!("RECEIPT hand={name} claims={claims} pages={open:?}");
                    }
                    Err(e) => pr!("ERROR cdp {e}"),
                }
            }
            "invoke" => {
                let a: Vec<&str> = rest.splitn(3, ' ').collect();
                if a.len() < 2 {
                    pr!("ERROR usage: invoke <handle> <selector> [invoke|toggle|expand|collapse|select|focus]");
                    continue;
                }
                let action = a.get(2).unwrap_or(&"invoke").to_string();
                let call = Call::Struct(StructArgs::Invoke { selector: a[1].into(), action });
                run_op(&mut runner, &mut relay, a[0], "invoke", call);
            }
            "page" | "win" => {
                // Register a browser page in the registry so ops can address
                // it. The in-memory body stays empty on purpose: a live
                // handle never reads it, and filling it would invite an op
                // to succeed against a model of a document instead of the
                // document.
                let a: Vec<&str> = rest.split_whitespace().collect();
                let (app, m, unit) = match a.as_slice() {
                    [app, m] => (*app, *m, ":doc"),
                    [app, m, u] => (*app, *m, *u),
                    _ => {
                        pr!("ERROR usage: page|win <app> <title-or-url-match> [unit]");
                        continue;
                    }
                };
                let h = new_handle(app, m, unit);
                relay.attach(&session, h.clone(), OpenFile::blank_word());
                pr!("RECEIPT attached={h}");
            }
            "hands" => {
                if !runner.has_hand() {
                    pr!("RECEIPT hands none");
                }
                for (name, apps) in runner.hands() {
                    let claims = if apps.is_empty() { "any".to_string() } else { apps.join(",") };
                    pr!("HAND {name} claims={claims}");
                }
            }
            "live" => {
                let h = rest.trim();
                if !relay.registry(&session).unwrap_or_default().contains(&h.to_string()) {
                    pr!("ERROR live {h} is not in the registry: attach it first");
                    continue;
                }
                if !runner.has_hand() {
                    pr!("ERROR live no hand: run `hand <pipe>` first");
                    continue;
                }
                match runner.mark_live(h) {
                    Ok(name) => pr!("RECEIPT live={h} hand={name} (ops on this handle now reach the open document)"),
                    Err(e) => pr!("ERROR live {e}"),
                }
            }
            "lread" => {
                // Queued like every other job, so a live read passes the
                // kill switch, the app allowlist and the doom-loop gate and
                // shows up in the event feed.
                let a: Vec<&str> = rest.splitn(2, ' ').collect();
                if a.len() < 2 {
                    pr!("ERROR usage: lread <handle> <selector>");
                    continue;
                }
                let call = Call::Read(ReadArgs { selector: a[1].into() });
                run_op(&mut runner, &mut relay, a[0], "lread", call);
            }
            "send" => {
                let a: Vec<&str> = rest.splitn(2, ' ').collect();
                if a.len() < 2 {
                    pr!("ERROR usage: send <skim|routine|code|deep|vision> <prompt>");
                    continue;
                }
                let task = match a[0] {
                    "skim" => TaskKind::Skim,
                    "routine" => TaskKind::Routine,
                    "code" => TaskKind::Code,
                    "deep" => TaskKind::DeepReasoning,
                    _ => TaskKind::VisionFallback,
                };
                let r = route(task);
                let base = config::base_url();
                let model = config::model_id(r.model);
                if !config::has_api_key() {
                    pr!("ERROR send env {} not set: add it to .env (see .env.example)", config::API_KEY_ENV);
                    continue;
                }
                let body = provider::request_body_with(&model, r, "You are the the agent desk worker. Answer briefly.", a[1]);
                match provider::send_via_curl(&base, config::API_KEY_ENV, &body) {
                    Ok((status, resp)) => {
                        pr!("RECEIPT send status={status} bytes={} model={model}", resp.len());
                        match provider::parse_chat_text(&resp) {
                            Some(t) => pr!("REPLY {}", t.replace('\n', " ")),
                            None if status != 200 => pr!("ERROR send body {}", resp.replace('\n', " ")),
                            None => pr!("REPLY (no content in response)"),
                        }
                    }
                    Err(e) => pr!("ERROR send {e}"),
                }
            }
            "pause" => {
                runner.pause();
                pr!("RECEIPT paused");
            }
            "resume" => match runner.resume(&relay) {
                Ok(missing) => pr!("RECEIPT resumed missing={missing:?}"),
                Err(e) => pr!("ERROR {e}"),
            },
            "pump" => match runner.pump(&mut relay) {
                Ok(o) => pr!("RECEIPT pump {o:?}"),
                Err(e) => pr!("ERROR pump {e}"),
            },
            "kill" => {
                runner.kill();
                pr!("RECEIPT killed");
            }
            "allow" => {
                let apps: Vec<String> = rest.split_whitespace().map(str::to_string).collect();
                if apps.is_empty() {
                    pr!("ERROR usage: allow <app...>");
                } else {
                    runner.lock_allowlist(apps.clone());
                    pr!("RECEIPT allowlist={apps:?}");
                }
            }
            "journal" => {
                if rest.is_empty() {
                    pr!("ERROR usage: journal <path> | journal off");
                } else if *rest == "off" {
                    journal_path = None;
                    pr!("RECEIPT journal=off");
                } else {
                    journal_path = Some(rest.to_string());
                    // Backfill everything since process start: the replay below
                    // re-attaches and re-runs without any prior manual setup.
                    let _ = std::fs::write(rest, history.join("\n") + "\n");
                    pr!("RECEIPT journal={rest}");
                }
            }
            "replay" => {
                if rest.is_empty() {
                    pr!("ERROR usage: replay <path>");
                    continue;
                }
                match std::fs::read_to_string(rest) {
                    Ok(text) => {
                        let lines: Vec<String> = text
                            .lines()
                            .map(str::trim)
                            .filter(|l| !l.is_empty() && !l.starts_with('#'))
                            .map(str::to_string)
                            .collect();
                        if lines.iter().any(|l| l.starts_with("replay ")) {
                            pr!("ERROR nested replay rejected");
                        } else {
                            for l in lines.into_iter().rev() {
                                buf.push_front(l);
                            }
                            pr!("RECEIPT replay={rest}");
                        }
                    }
                    Err(e) => pr!("ERROR replay {e}"),
                }
            }
            _ => pr!("ERROR unknown command {cmd:?}"),
        }
    }
}
