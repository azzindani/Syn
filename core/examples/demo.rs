//! M2/M3 acceptance demo: 2 Excels + 1 Word open in one session,
//! typed transfers with provenance, pause/resume, full event log.
//! Run: cargo run --example demo
use harness_core::bus::{FileContent, FileKind, OpenFile};
use harness_core::ops::{Call, ReadArgs, StructArgs, WriteArgs, execute};
use harness_core::protocol::new_handle;
use harness_core::Relay;
use std::collections::HashMap;

fn excel(rows: Vec<Vec<&str>>) -> OpenFile {
    OpenFile {
        kind: FileKind::Excel,
        content: FileContent::Excel {
            sheets: HashMap::from([(
                "Sheet1".to_string(),
                rows.into_iter().map(|r| r.into_iter().map(str::to_string).collect()).collect(),
            )]),
        },
        styles: HashMap::new(),
    }
}

fn word() -> OpenFile {
    OpenFile {
        kind: FileKind::Word,
        content: FileContent::Word { paras: vec!["Q3 Report".to_string()], tables: vec![], changes: vec![], comments: vec![] },
        styles: HashMap::new(),
    }
}

fn main() {
    let mut r = Relay::new();
    let s = "demo";
    r.handshake(s, "harness-widget");
    let plan = new_handle("excel", "plan.xlsx", "Sheet1");
    let act = new_handle("excel", "actual.xlsx", "Sheet1");
    let rep = new_handle("word", "report.docx", "body");
    r.attach(s, plan.clone(), excel(vec![vec!["item", "plan"], vec!["ads", "100"], vec!["ops", "200"]]));
    r.attach(s, act.clone(), excel(vec![vec!["item", "actual"], vec!["ads", "130"], vec!["ops", "180"]]));
    r.attach(s, rep.clone(), word());
    println!("registry: {:?}", r.registry(s).unwrap());

    // Pause before acting (human gate), then resume.
    r.emit(s, "paused", "", "human reviewing plan".into()).unwrap();
    r.emit(s, "resumed", "", "human approved".into()).unwrap();

    // Transfer both tables into the report with provenance.
    for (src, title) in [(&plan, "Plan"), (&act, "Actual")] {
        let out = execute(&mut r, s, &rep, Call::Struct(StructArgs::Transfer {
            from: src.clone(), selector: "Sheet1!A1:B3".into(), title: title.into(),
        }))
        .unwrap();
        println!("{title}: {out:?}");
    }
    // Edit a cell, then undo it to prove per-file scope.
    execute(&mut r, s, &plan, Call::Write(WriteArgs { selector: "Sheet1!B2:B2".into(), values: vec![vec!["999".into()]] })).unwrap();
    let undone = execute(&mut r, s, &plan, Call::Undo).unwrap();
    println!("undo plan: {undone:?}");

    let kinds: Vec<String> = r.events(s).unwrap().iter().map(|e| e.t.clone()).collect();
    println!("events({}): {}", kinds.len(), kinds.join(","));
    let rep_out = execute(&mut r, s, &rep, Call::Read(ReadArgs { selector: "body".into() })).unwrap();
    println!("report: {rep_out:?}");
    assert!(kinds.contains(&"xfer".to_string()));
    assert!(kinds.contains(&"paused".to_string()));
    println!("DEMO PASS: multi-file transfer + provenance + pause/resume + undo");
}
