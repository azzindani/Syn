//! harness-cli: scriptable REPL over the Runner. Every line is one command;
//! every mutation prints a RECEIPT line. Pipe a script file for E2E tests.
//!
//! Commands:
//!   session <id>            handshake (default session `cli`)
//!   attach excel <f> <sh>   | attach word <f> | attach ppt <f>
//!   read <handle> <sel>     | write <h> <sel> <r1c1,r1c2;r2c1> | para <h> <text>
//!   slide <h> <title> <b1|b2> | xfer <src> <sel> <dst> <title>
//!   undo <h> | events | registry | route <skim|routine|code|deep|vision>
//!   pause | resume | pump | kill | allow <app...> | send <task> <prompt...> (provider POST, needs key)
//!   quit

use harness_core::bus::{FileContent, FileKind, OpenFile};
use harness_core::ops::{Call, ExportArgs, FormatArgs, ReadArgs, StructArgs, WriteArgs, execute};
use harness_core::protocol::new_handle;
use harness_core::provider::{self, request_body};
use harness_core::router::{TaskKind, route};
use harness_core::runner::{Job, Runner};
use harness_core::Relay;
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
    let mut relay = Relay::new();
    let mut session = "cli".to_string();
    let mut runner = Runner::new(&session);
    relay.handshake(&session, "harness-cli");

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
                relay.handshake(&session, "harness-cli");
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
                    Ok(o) => println!("RECEIPT {id} {o:?}"),
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
                println!("RECEIPT route model={:?} effort={:?} rank={}", r.model, r.effort, r.cost_rank);
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
                let base = std::env::var("HARNESS_BASE_URL").unwrap_or_else(|_| provider::DEFAULT_BASE_URL.into());
                let body = request_body(r, "You are the Rig desk worker. Answer briefly.", a[1]);
                match provider::send_via_curl(&base, "HARNESS_API_KEY", &body) {
                    Ok((status, resp)) => println!("RECEIPT send status={status} bytes={} model={:?}", resp.len(), r.model),
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
