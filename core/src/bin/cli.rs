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
//!   hand <pipe>             connect to a live office-host sidecar
//!   live <handle>           route that handle's ops to the open document
//!   lread <h> <sel>         queued read of a live handle
//!                           (once `live`, plain `write` also hits the document)
//!   quit

use core::bus::{FileContent, FileKind, OpenFile};
use core::config;
use core::hand::Hand;
use core::ops::{Call, ExportArgs, FormatArgs, ReadArgs, StructArgs, WriteArgs, execute};
use core::protocol::new_handle;
use core::provider;
use core::router::{TaskKind, route};
use core::runner::{Job, Runner};
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
    // Every live line since process start: enabling the journal backfills
    // this so a replay is always self-contained (replay/session lines kept,
    // nested replay lines skipped).
    let mut history: Vec<String> = Vec::new();
    loop {
        let (line, live) = match buf.pop_front() {
            Some(l) => (l, false),
            None => match stdin.lock().lines().next() {
                Some(Ok(l)) => (l, true),
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
            "registry" => println!("RECEIPT registry={:?}", relay.registry(&session).unwrap_or_default()),
            "read" => {
                let a: Vec<&str> = rest.splitn(2, ' ').collect();
                if a.len() < 2 {
                    println!("ERROR usage: read <handle> <selector>");
                    continue;
                }
                match execute(&mut relay, &session, a[0], Call::Read(ReadArgs { selector: a[1].into() })) {
                    Ok(o) => println!("RECEIPT read {o:?}"),
                    Err(e) => println!("ERROR {e}"),
                }
            }
            "write" => {
                let a: Vec<&str> = rest.splitn(3, ' ').collect();
                if a.len() < 3 {
                    println!("ERROR usage: write <handle> <selector> <v11,v12;r21>");
                    continue;
                }
                let call = Call::Write(WriteArgs { selector: a[1].into(), values: parse_grid(a[2]) });
                let h = a[0].to_string();
                let id = runner.submit(Job { handle: h, summary: "cli-write".into(), call });
                match runner.pump(&mut relay) {
                    Ok(Some(o)) => println!("RECEIPT {id} {o:?}"),
                    Ok(None) => println!("ERROR {id} queue not running"),
                    Err(e) => println!("ERROR {id} {e}"),
                }
            }
            "para" => {
                let a: Vec<&str> = rest.splitn(2, ' ').collect();
                if a.len() < 2 {
                    println!("ERROR usage: para <handle> <text>");
                    continue;
                }
                match execute(&mut relay, &session, a[0], Call::Struct(StructArgs::InsertParagraph { text: a[1].into() })) {
                    Ok(o) => println!("RECEIPT para {o:?}"),
                    Err(e) => println!("ERROR {e}"),
                }
            }
            "slide" => {
                let a: Vec<&str> = rest.splitn(3, ' ').collect();
                if a.len() < 3 {
                    println!("ERROR usage: slide <handle> <title> <b1|b2>");
                    continue;
                }
                let bullets = a[2].split('|').map(str::to_string).collect();
                match execute(&mut relay, &session, a[0], Call::Struct(StructArgs::CreateSlide { title: a[1].into(), bullets })) {
                    Ok(o) => println!("RECEIPT slide {o:?}"),
                    Err(e) => println!("ERROR {e}"),
                }
            }
            "xfer" => {
                let a: Vec<&str> = rest.splitn(4, ' ').collect();
                if a.len() < 4 {
                    println!("ERROR usage: xfer <src> <selector> <dst> <title>");
                    continue;
                }
                let call = Call::Struct(StructArgs::Transfer { from: a[0].into(), selector: a[1].into(), title: a[3].into() });
                let h = a[2].to_string();
                let id = runner.submit(Job { handle: h, summary: "cli-xfer".into(), call });
                match runner.pump(&mut relay) {
                    Ok(o) => println!("RECEIPT {id} {o:?}"),
                    Err(e) => println!("ERROR {id} {e}"),
                }
            }
            "undo" => match execute(&mut relay, &session, rest, Call::Undo) {
                Ok(o) => println!("RECEIPT undo {o:?}"),
                Err(e) => println!("ERROR {e}"),
            },
            "export" => {
                let a: Vec<&str> = rest.splitn(3, ' ').collect();
                let fmt = a.get(1).unwrap_or(&"summary").to_string();
                let path = a.get(2).map(|s| s.to_string());
                match execute(&mut relay, &session, a[0], Call::Export(ExportArgs { format: fmt, path, sheet: None })) {
                    Ok(o) => println!("RECEIPT export {o:?}"),
                    Err(e) => println!("ERROR {e}"),
                }
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
                match execute(&mut relay, &session, a[0], Call::Format(FormatArgs { selector: a[1].into(), style })) {
                    Ok(o) => println!("RECEIPT format {o:?}"),
                    Err(e) => println!("ERROR {e}"),
                }
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
            "hand" => {
                let pipe = rest.trim();
                match Hand::connect(pipe) {
                    Ok(h) => {
                        runner.attach_hand(Box::new(h));
                        println!("RECEIPT hand connected pipe={pipe}");
                    }
                    Err(e) => println!("ERROR hand {e}"),
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
                runner.mark_live(h);
                println!("RECEIPT live={h} (ops on this handle now reach the open document)");
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
                let id = runner.submit(Job { handle: a[0].into(), summary: "lread".into(), call });
                match runner.pump(&mut relay) {
                    Ok(Some(o)) => println!("RECEIPT {id} {o:?}"),
                    Ok(None) => println!("ERROR {id} queue not running"),
                    Err(e) => println!("ERROR {id} {e}"),
                }
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
