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
//!   do <goal>               run the agent loop until it answers or stops
//!   approve | deny [why]    answer a held shell confirmation
//!   quit

use core::agent::{Agent, Brain, CurlBrain, Step};
use core::bus::{FileContent, FileKind, OpenFile};
use core::config;
use core::cdp::Cdp;
use core::chats;
use core::hand::Hand;
use core::ops::{Call, ExportArgs, FormatArgs, ReadArgs, StructArgs, WriteArgs};
use core::protocol::new_handle;
use core::provider;
use core::router::{TaskKind, route};
use core::runner::{Job, Runner};
use core::shell::ShellPolicy;
use core::Relay;
use std::collections::HashMap;
use std::io::BufRead;

fn blank_excel() -> OpenFile {
    OpenFile {
        kind: FileKind::Excel,
        content: FileContent::Excel {
            sheets: HashMap::from([("Sheet1".into(), vec![vec![String::new(); 4]; 4])]),
        },
        styles: HashMap::new(),
    }
}

/// Submit one op and run it.
///
/// Every op arm goes through here. Calling `ops::execute` from a command
/// would skip the kill switch, the app allowlist, the doom-loop gate, the
/// event feed AND the live-hand routing, so a handle marked live would
/// quietly edit the in-memory model instead of the open document. The agent
/// loop already holds this invariant; the REPL now holds it too.
fn run_op(runner: &mut Runner, relay: &mut Relay, handle: &str, what: &str, call: Call) {
    let id = runner.submit(Job { handle: handle.into(), summary: format!("cli-{what}"), call });
    match runner.pump(relay) {
        Ok(Some(o)) => println!("RECEIPT {id} {what} {o:?}"),
        Ok(None) => println!("ERROR {id} queue not running"),
        Err(e) => println!("ERROR {id} {e}"),
    }
}

fn blank_word() -> OpenFile {
    OpenFile {
        kind: FileKind::Word,
        content: FileContent::Word { paras: vec![], tables: vec![], changes: vec![], comments: vec![] },
        styles: HashMap::new(),
    }
}

fn blank_ppt() -> OpenFile {
    OpenFile {
        kind: FileKind::Ppt, content: FileContent::Ppt { slides: vec![] }, styles: HashMap::new(),
    }
}

/// Drive the loop until it finishes, stops, or stops for a human.
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
        println!("ERROR chat save {e}");
    }
}

fn drive(
    a: &mut Agent,
    brain: &mut dyn Brain,
    relay: &mut core::Relay,
    runner: &mut Runner,
    sp: &ShellPolicy,
) {
    loop {
        match a.step(brain, relay, runner, sp) {
            Step::Ran { tool, detail } => println!("STEP {tool}: {detail}"),
            Step::Refused(why) => println!("REFUSED {why}"),
            Step::Answered(text) => {
                println!("ANSWER {}", text.replace('\n', " "));
                break;
            }
            Step::Stopped(why) => {
                println!("STOPPED {why}");
                break;
            }
            Step::NeedsApproval(p) => {
                println!("CONFIRM {}", p.preview);
                println!("        reason given: {}", p.why);
                println!("        respond with `approve` or `deny <reason>`");
                break;
            }
        }
    }
}

fn parse_grid(s: &str) -> Vec<Vec<String>> {
    s.split(';').map(|r| r.split(',').map(str::to_string).collect()).collect()
}

fn main() {
    // Deployment config before anything else: .env seeds the process env,
    // a real shell export always wins. Key names only are printed.
    if let Some(l) = config::load_env(&std::env::current_dir().unwrap_or_default()) {
        println!("RECEIPT env file={} applied={} kept={}", l.path.display(), l.applied.join(","), l.skipped.join(","));
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
            "session" => {
                session = rest.to_string();
                relay.handshake(&session, "cli");
                runner = Runner::new(&session);
                println!("RECEIPT session={session}");
            }
            "attach" => {
                let a: Vec<&str> = rest.split_whitespace().collect();
                let (file, kind) = match a.as_slice() {
                    ["excel", f, unit] => (blank_excel(), new_handle("excel", f, unit)),
                    ["word", f] => (blank_word(), new_handle("word", f, "body")),
                    ["ppt", f] => (blank_ppt(), new_handle("ppt", f, "deck")),
                    _ => {
                        println!("ERROR usage: attach excel <file> <sheet> | attach word <file> | attach ppt <file>");
                        continue;
                    }
                };
                relay.attach(&session, kind.clone(), file);
                println!("RECEIPT attached={kind}");
            }
            // A sentinel so a non-interactive driver knows where one
            // command's output ends. Commands print a variable number of
            // lines, so a reader with no marker either guesses or blocks.
            "mark" => println!("RECEIPT mark={}", rest.trim()),
            "registry" => println!("RECEIPT registry={:?}", relay.registry(&session).unwrap_or_default()),
            "read" => {
                let a: Vec<&str> = rest.splitn(2, ' ').collect();
                if a.len() < 2 {
                    println!("ERROR usage: read <handle> <selector>");
                    continue;
                }
                run_op(&mut runner, &mut relay, a[0], "read", Call::Read(ReadArgs { selector: a[1].into() }));
            }
            "write" => {
                let a: Vec<&str> = rest.splitn(3, ' ').collect();
                if a.len() < 3 {
                    println!("ERROR usage: write <handle> <selector> <v11,v12;r21>");
                    continue;
                }
                let call = Call::Write(WriteArgs { selector: a[1].into(), values: parse_grid(a[2]) });
                run_op(&mut runner, &mut relay, a[0], "write", call);
            }
            "para" => {
                let a: Vec<&str> = rest.splitn(2, ' ').collect();
                if a.len() < 2 {
                    println!("ERROR usage: para <handle> <text>");
                    continue;
                }
                let call = Call::Struct(StructArgs::InsertParagraph { text: a[1].into() });
                run_op(&mut runner, &mut relay, a[0], "para", call);
            }
            "slide" => {
                let a: Vec<&str> = rest.splitn(3, ' ').collect();
                if a.len() < 3 {
                    println!("ERROR usage: slide <handle> <title> <b1|b2>");
                    continue;
                }
                let bullets = a[2].split('|').map(str::to_string).collect();
                let call = Call::Struct(StructArgs::CreateSlide { title: a[1].into(), bullets });
                run_op(&mut runner, &mut relay, a[0], "slide", call);
            }
            "xfer" => {
                let a: Vec<&str> = rest.splitn(4, ' ').collect();
                if a.len() < 4 {
                    println!("ERROR usage: xfer <src> <selector> <dst> <title>");
                    continue;
                }
                let call = Call::Struct(StructArgs::Transfer { from: a[0].into(), selector: a[1].into(), title: a[3].into() });
                run_op(&mut runner, &mut relay, a[2], "xfer", call);
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
                    println!("ERROR usage: format <handle> <selector> <k=v,k=v>");
                    continue;
                }
                let mut style = Vec::new();
                for kv in a[2].split(',') {
                    match kv.split_once('=') {
                        Some((k, v)) => style.push((k.to_string(), v.to_string())),
                        None => {
                            println!("ERROR bad style pair {kv:?}, want k=v");
                            continue;
                        }
                    }
                }
                let call = Call::Format(FormatArgs { selector: a[1].into(), style });
                run_op(&mut runner, &mut relay, a[0], "format", call);
            }
            "events" => {
                for e in relay.events(&session).unwrap_or_default() {
                    println!("EVENT {} {} {}", e.t, e.handle, e.detail);
                }
                println!("RECEIPT events-end");
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
                println!(
                    "RECEIPT route model={:?} id={} effort={:?} rank={}",
                    r.model,
                    config::model_id(r.model),
                    r.effort,
                    r.cost_rank
                );
            }
            "config" => println!("RECEIPT config {}", config::describe()),
            "task" => {
                task = match rest.trim() {
                    "skim" => TaskKind::Skim,
                    "routine" => TaskKind::Routine,
                    "code" => TaskKind::Code,
                    "deep" => TaskKind::DeepReasoning,
                    "vision" => TaskKind::VisionFallback,
                    other => {
                        println!("ERROR task: unknown class {other:?} (skim|routine|code|deep|vision)");
                        continue;
                    }
                };
                let r = route(task);
                println!("RECEIPT task={:?} model={}", task, config::model_id(r.model));
            }
            "shellallow" => {
                let progs: Vec<&str> = rest.split_whitespace().collect();
                shell_policy = ShellPolicy::new(&progs);
                println!("RECEIPT shell-allowlist={:?}", shell_policy.allowed());
            }
            "do" => {
                if rest.trim().is_empty() {
                    println!("ERROR usage: do <goal>");
                    continue;
                }
                if !config::has_api_key() {
                    println!("ERROR do: env {} not set (see .env.example)", config::API_KEY_ENV);
                    continue;
                }
                let r = route(task);
                let model = config::model_id(r.model);
                let mut brain = CurlBrain { base_url: config::base_url(), api_key_env: config::API_KEY_ENV.into() };
                let mut a = Agent::new(&session, rest.trim(), &model, r);
                println!("RECEIPT do model={model} max_steps={}", a.max_steps);
                drive(&mut a, &mut brain, &mut relay, &mut runner, &shell_policy);
                agent = Some(a);
            }
            "say" => {
                // One conversational turn. Unlike `do`, this keeps the
                // transcript: a chat where every message starts a new agent
                // is not a chat, it is a series of strangers.
                let text = rest.trim();
                if text.is_empty() {
                    println!("ERROR usage: say <text>");
                    continue;
                }
                if !config::has_api_key() {
                    println!("ERROR say: env {} not set (see .env.example)", config::API_KEY_ENV);
                    continue;
                }
                let r = route(task);
                let model = config::model_id(r.model);
                match agent.as_mut() {
                    Some(a) => {
                        if let Err(e) = a.follow_up(text) {
                            println!("ERROR say {e}");
                            continue;
                        }
                        // Apply the current slot every turn, so switching
                        // after a 429 actually moves the next request.
                        a.retarget(&model, r);
                    }
                    None => agent = Some(Agent::new(&session, text, &model, r)),
                }
                let a = agent.as_mut().expect("just set");
                println!("RECEIPT say model={model}");
                let mut brain = CurlBrain { base_url: config::base_url(), api_key_env: config::API_KEY_ENV.into() };
                drive(a, &mut brain, &mut relay, &mut runner, &shell_policy);
                save_chat(&chat_id, a);
            }
            "chat" => {
                let mut a = rest.split_whitespace();
                match a.next().unwrap_or("msgs") {
                    "new" => {
                        chat_id = chats::new_id();
                        agent = None;
                        println!("RECEIPT chat={chat_id}");
                    }
                    "list" => {
                        for m in chats::list() {
                            println!("CHAT {}", chats::meta_json(&m));
                        }
                        println!("RECEIPT chat current={chat_id}");
                    }
                    "open" => {
                        let Some(id) = a.next() else {
                            println!("ERROR usage: chat open <id>");
                            continue;
                        };
                        match chats::load(id) {
                            Ok(c) => {
                                let r = route(task);
                                agent = Some(Agent::resume(&session, c.msgs, &config::model_id(r.model), r));
                                chat_id = c.meta.id;
                                println!("RECEIPT chat={chat_id} title={:?}", c.meta.title);
                            }
                            Err(e) => println!("ERROR chat open {e}"),
                        }
                    }
                    "del" => {
                        let Some(id) = a.next() else {
                            println!("ERROR usage: chat del <id>");
                            continue;
                        };
                        match chats::delete(id) {
                            Ok(()) => {
                                if id == chat_id {
                                    chat_id = chats::new_id();
                                    agent = None;
                                }
                                println!("RECEIPT deleted={id}");
                            }
                            Err(e) => println!("ERROR chat del {e}"),
                        }
                    }
                    "msgs" => {
                        // The transcript as the UI wants it, plus whatever
                        // the run is currently waiting on, so the page never
                        // has to infer state from receipt text.
                        if let Some(ag) = agent.as_ref() {
                            for m in ag.transcript() {
                                println!("MSG {}", chats::msg_json(m));
                            }
                            if let Some(p) = ag.pending() {
                                println!(
                                    "PENDING {{\"program\":{:?},\"preview\":{:?},\"why\":{:?}}}",
                                    p.program, p.preview, p.why
                                );
                            }
                        }
                        println!("RECEIPT chat={chat_id}");
                    }
                    other => println!("ERROR chat: unknown {other:?}, want new|list|open|del|msgs"),
                }
            }
            "approve" | "deny" => {
                let Some(a) = agent.as_mut() else {
                    println!("ERROR {cmd}: no run in progress");
                    continue;
                };
                let outcome = if cmd == "approve" {
                    a.approve(&mut relay, &shell_policy)
                } else {
                    a.deny(&mut relay, rest.trim())
                };
                match &outcome {
                    Step::Ran { tool, detail } => println!("STEP {tool}: {detail}"),
                    Step::Refused(why) => println!("REFUSED {why}"),
                    other => println!("RECEIPT {other:?}"),
                }
                if !matches!(outcome, Step::Stopped(_)) {
                    let mut brain = CurlBrain { base_url: config::base_url(), api_key_env: config::API_KEY_ENV.into() };
                    drive(a, &mut brain, &mut relay, &mut runner, &shell_policy);
                }
                save_chat(&chat_id, a);
            }
            "hand" => {
                // `hand <pipe> [app...]`: with apps, this hand claims them;
                // with none it is the catch-all for handles no other hand
                // claims. Several hands can be attached at once.
                let mut parts = rest.split_whitespace();
                let Some(pipe) = parts.next() else {
                    println!("ERROR usage: hand <pipe> [app...]");
                    continue;
                };
                let apps: Vec<String> = parts.map(str::to_string).collect();
                match Hand::connect(pipe) {
                    Ok(h) => {
                        let claims = if apps.is_empty() { "any".to_string() } else { apps.join(",") };
                        runner.attach_hand_as(pipe, apps, Box::new(h));
                        println!("RECEIPT hand={pipe} claims={claims}");
                    }
                    Err(e) => println!("ERROR hand {e}"),
                }
            }
            "cdp" => {
                // One hand, every Chromium on the box: browsers and every
                // Electron app started with --remote-debugging-port.
                let mut parts = rest.split_whitespace();
                let Some(addr) = parts.next() else {
                    println!("ERROR usage: cdp <host:port> [app...]");
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
                        println!("RECEIPT hand={name} claims={claims} pages={open:?}");
                    }
                    Err(e) => println!("ERROR cdp {e}"),
                }
            }
            "invoke" => {
                let a: Vec<&str> = rest.splitn(3, ' ').collect();
                if a.len() < 2 {
                    println!("ERROR usage: invoke <handle> <selector> [invoke|toggle|expand|collapse|select|focus]");
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
                        println!("ERROR usage: page|win <app> <title-or-url-match> [unit]");
                        continue;
                    }
                };
                let h = new_handle(app, m, unit);
                relay.attach(&session, h.clone(), blank_word());
                println!("RECEIPT attached={h}");
            }
            "hands" => {
                if !runner.has_hand() {
                    println!("RECEIPT hands none");
                }
                for (name, apps) in runner.hands() {
                    let claims = if apps.is_empty() { "any".to_string() } else { apps.join(",") };
                    println!("HAND {name} claims={claims}");
                }
            }
            "live" => {
                let h = rest.trim();
                if !relay.registry(&session).unwrap_or_default().contains(&h.to_string()) {
                    println!("ERROR live {h} is not in the registry: attach it first");
                    continue;
                }
                if !runner.has_hand() {
                    println!("ERROR live no hand: run `hand <pipe>` first");
                    continue;
                }
                match runner.mark_live(h) {
                    Ok(name) => println!("RECEIPT live={h} hand={name} (ops on this handle now reach the open document)"),
                    Err(e) => println!("ERROR live {e}"),
                }
            }
            "lread" => {
                // Queued like every other job, so a live read passes the
                // kill switch, the app allowlist and the doom-loop gate and
                // shows up in the event feed.
                let a: Vec<&str> = rest.splitn(2, ' ').collect();
                if a.len() < 2 {
                    println!("ERROR usage: lread <handle> <selector>");
                    continue;
                }
                let call = Call::Read(ReadArgs { selector: a[1].into() });
                run_op(&mut runner, &mut relay, a[0], "lread", call);
            }
            "send" => {
                let a: Vec<&str> = rest.splitn(2, ' ').collect();
                if a.len() < 2 {
                    println!("ERROR usage: send <skim|routine|code|deep|vision> <prompt>");
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
                    println!("ERROR send env {} not set: add it to .env (see .env.example)", config::API_KEY_ENV);
                    continue;
                }
                let body = provider::request_body_with(&model, r, "You are the Syn desk worker. Answer briefly.", a[1]);
                match provider::send_via_curl(&base, config::API_KEY_ENV, &body) {
                    Ok((status, resp)) => {
                        println!("RECEIPT send status={status} bytes={} model={model}", resp.len());
                        match provider::parse_chat_text(&resp) {
                            Some(t) => println!("REPLY {}", t.replace('\n', " ")),
                            None if status != 200 => println!("ERROR send body {}", resp.replace('\n', " ")),
                            None => println!("REPLY (no content in response)"),
                        }
                    }
                    Err(e) => println!("ERROR send {e}"),
                }
            }
            "pause" => {
                runner.pause();
                println!("RECEIPT paused");
            }
            "resume" => match runner.resume(&relay) {
                Ok(missing) => println!("RECEIPT resumed missing={missing:?}"),
                Err(e) => println!("ERROR {e}"),
            },
            "pump" => match runner.pump(&mut relay) {
                Ok(o) => println!("RECEIPT pump {o:?}"),
                Err(e) => println!("ERROR pump {e}"),
            },
            "kill" => {
                runner.kill();
                println!("RECEIPT killed");
            }
            "allow" => {
                let apps: Vec<String> = rest.split_whitespace().map(str::to_string).collect();
                if apps.is_empty() {
                    println!("ERROR usage: allow <app...>");
                } else {
                    runner.lock_allowlist(apps.clone());
                    println!("RECEIPT allowlist={apps:?}");
                }
            }
            "journal" => {
                if rest.is_empty() {
                    println!("ERROR usage: journal <path> | journal off");
                } else if *rest == "off" {
                    journal_path = None;
                    println!("RECEIPT journal=off");
                } else {
                    journal_path = Some(rest.to_string());
                    // Backfill everything since process start: the replay below
                    // re-attaches and re-runs without any prior manual setup.
                    let _ = std::fs::write(rest, history.join("\n") + "\n");
                    println!("RECEIPT journal={rest}");
                }
            }
            "replay" => {
                if rest.is_empty() {
                    println!("ERROR usage: replay <path>");
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
                            println!("ERROR nested replay rejected");
                        } else {
                            for l in lines.into_iter().rev() {
                                buf.push_front(l);
                            }
                            println!("RECEIPT replay={rest}");
                        }
                    }
                    Err(e) => println!("ERROR replay {e}"),
                }
            }
            _ => println!("ERROR unknown command {cmd:?}"),
        }
    }
}
