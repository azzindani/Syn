//! What a live application's "no" means, and what to do about it.
//!
//! A refusal from our own tool layer already says how to fix the call. A
//! refusal from the application does not: office-host hands back what COM
//! said, and COM says `com 0x800A03EC: Exception from HRESULT: 0x800A03EC`.
//! A frontier model has seen that code and guesses; a small one retries the
//! identical call until the repeat gate stops it. Either way the step is
//! spent learning nothing.
//!
//! This is the layer that turns the known failures of a live Windows
//! application into one sentence of cause and one of action. It adds, it
//! never replaces: the application's own words stay in front of the advice,
//! because they are the evidence and a model should see them.
//!
//! Every rule keys on text a helper really sends. Where that text is ours
//! (office-host, uia-host, the browser hand), `core/tests/coach_contract.rs`
//! checks it still appears in that source, so rewording an error cannot
//! quietly switch its advice off. COM's own codes are Windows', and are
//! listed with what they mean.
//!
//! The advice describes the situation and the way out. It never says what
//! the job is, and where the way out needs a person (a dialog, a protected
//! sheet, a closed window) it says to ask one rather than to work around
//! them: they are looking at the screen, and the application is theirs.

/// Who the advice is for: a model in Syn's own loop, which can ask the
/// human in prose but cannot open files, or one behind MCP, which can.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    Loop,
    Mcp,
}

/// One known failure.
pub struct Rule {
    /// Text in the error that identifies it. Matched without case.
    pub needle: &'static str,
    /// Where that text comes from: a source file (relative to the repo
    /// root) whose wording it tracks, or `"com"` for a Windows code.
    pub from: &'static str,
    /// Apps it applies to; empty for any.
    pub apps: &'static [&'static str],
    pub advice: &'static str,
    /// Different advice behind MCP, where `open` exists; `None` for same.
    pub mcp: Option<&'static str>,
}

const HOST: &str = "sidecar-csharp/Host/Program.cs";
const UIA: &str = "sidecar-csharp/Uia/Program.cs";
const CDP: &str = "core/src/cdp.rs";

/// First match wins, so the specific come before the general.
pub const RULES: &[Rule] = &[
    // ---- the person is in the way, or the app is busy --------------------
    Rule {
        needle: "modal dialog or busy app",
        from: HOST,
        apps: &[],
        advice: "A dialog is open in the application or it is busy (a cell still being edited, a save prompt, an error box). Nothing was changed. Ask the user to close the dialog or press Esc, then repeat the call. Do not retry before they answer.",
        mcp: None,
    },
    Rule {
        // VBA_E_IGNORE: Excel is refusing automation, almost always because
        // a cell is in edit mode or a dialog has focus.
        needle: "0x800AC472",
        from: "com",
        apps: &["excel"],
        advice: "Excel is busy: usually a cell is still being edited or a dialog is open. Ask the user to press Esc in Excel, then repeat the call.",
        mcp: None,
    },
    // ---- the application went away -------------------------------------
    Rule {
        // RPC_S_SERVER_UNAVAILABLE: the process is gone.
        needle: "0x800706BA",
        from: "com",
        apps: &[],
        advice: "The application was closed or crashed. Nothing more can reach it until it is opened again. Ask the user whether they closed it, and to open the document again if the work should go on.",
        mcp: Some("The application was closed or crashed. Call `open` with the document's full path to start it again; if the user closed it on purpose, ask them first."),
    },
    Rule {
        // RPC_E_DISCONNECTED: the object this call held was released,
        // typically because the document or the app was closed.
        needle: "0x80010108",
        from: "com",
        apps: &[],
        advice: "The document or application this handle pointed at was closed. Ask the user to open it again; do not guess another handle.",
        mcp: Some("The document or application this handle pointed at was closed. Call `open` with the document's full path to get a working handle."),
    },
    Rule {
        // RPC_S_CALL_FAILED: the app died mid-call.
        needle: "0x800706BE",
        from: "com",
        apps: &[],
        advice: "The application stopped responding in the middle of the call and may have crashed. Ask the user to check it; whatever that call was doing may be half done, so read the target before writing again.",
        mcp: Some("The application stopped responding in the middle of the call and may have crashed. Call `open` again, then read the target before writing: the call may be half done."),
    },
    Rule {
        needle: "workbook not open for",
        from: HOST,
        apps: &["excel"],
        advice: "That workbook is not open in Excel any more: the user closed it, or it is open under another name. Ask them to open it again; do not guess another name.",
        mcp: Some("That workbook is not open in Excel any more: the user closed it, or it is open under another name. Call `open` with its full path."),
    },
    Rule {
        needle: "doc not open for",
        from: HOST,
        apps: &["word"],
        advice: "That document is not open in Word any more: the user closed it, or it is open under another name. Ask them to open it again; do not guess another name.",
        mcp: Some("That document is not open in Word any more: the user closed it, or it is open under another name. Call `open` with its full path."),
    },
    Rule {
        needle: "presentation not open for",
        from: HOST,
        apps: &["ppt"],
        advice: "That presentation is not open in PowerPoint any more: the user closed it, or it is open under another name. Ask them to open it again; do not guess another name.",
        mcp: Some("That presentation is not open in PowerPoint any more: the user closed it, or it is open under another name. Call `open` with its full path."),
    },
    Rule {
        needle: "no such file",
        from: HOST,
        apps: &["excel", "word", "ppt"],
        advice: "No file exists at that path. Check the spelling and the extension, and give the full path, like C:\\Users\\name\\Documents\\report.xlsx. Nothing here creates a file by opening it.",
        mcp: None,
    },
    // ---- the document will not let us ------------------------------------
    Rule {
        // Excel's words for a write into a locked cell.
        needle: "protected sheet",
        from: "com",
        apps: &["excel"],
        advice: "That sheet is protected against changes. Ask the user to unprotect it (Review > Unprotect Sheet) or to say where you may write instead.",
        mcp: None,
    },
    Rule {
        needle: "read-only",
        from: "com",
        apps: &["excel", "word", "ppt"],
        advice: "The file is open read-only: often it came from an email or the internet and is in Protected View. Ask the user to click Enable Editing, then repeat the call.",
        mcp: None,
    },
    Rule {
        // Word's "Command failed" (error 4198).
        needle: "0x800A1066",
        from: "com",
        apps: &["word"],
        advice: "Word refused the command. The usual causes: the document is protected or in Protected View (ask the user to enable editing), or the paragraph is past the end (read `body` for the count; p0 is the first).",
        mcp: None,
    },
    Rule {
        // "Unable to set the Orientation property of the PageSetup class":
        // Excel asks the printer driver for every page setting, and a
        // machine with no printer installed refuses them all.
        needle: "PageSetup class",
        from: "com",
        apps: &["excel"],
        advice: "Excel could not change the page setup. It asks the printer for every page setting, so this usually means no printer is installed; ask the user to add one (Microsoft Print to PDF is enough), then repeat the call.",
        mcp: None,
    },
    Rule {
        needle: "Sort method of Range class failed",
        from: "com",
        apps: &["excel"],
        advice: "Excel could not sort that range. The usual causes: merged cells in it (unmerge them first), or a range that is only part of a table; sort the whole block, header row first.",
        mcp: None,
    },
    Rule {
        // NAME_NOT_FOUND, which Excel uses for most refusals it does not
        // explain. Last among the Excel rules because it is the vaguest.
        needle: "0x800A03EC",
        from: "com",
        apps: &["excel"],
        advice: "Excel rejected the call without saying why. The usual causes, most likely first: a formula it cannot parse (write formulas in English with commas between arguments, like =SUMIF(A:A,\"x\",B:B), whatever language the computer uses); a sheet or range that does not exist; a protected sheet. Read the target to check before changing anything, and do not repeat the same call.",
        mcp: None,
    },
    Rule {
        // DISP_E_TYPEMISMATCH.
        needle: "0x80020005",
        from: "com",
        apps: &[],
        advice: "A value was the wrong type for the application (text where it wanted a number or true/false, or the other way round). Check the call's values against the tool's description.",
        mcp: None,
    },
    // ---- windows (UI Automation) -----------------------------------------
    Rule {
        needle: "no open window matches",
        from: UIA,
        apps: &["ui"],
        advice: "No window's title contains that text now. Titles change when a document is saved or switched, or a page changes. Ask the user which window they mean.",
        mcp: Some("No window's title contains that text now; titles change when a document is saved or a page changes. Call `open` with app window and part of the title as it reads in the title bar now."),
    },
    Rule {
        needle: "no control matches",
        from: UIA,
        apps: &["ui"],
        advice: "Nothing in that window matches the selector. Read it with selector :tree to see its controls with their type, id= and name=, then address one of those. Prefer id= when a control has one; name= also matches part of a name.",
        mcp: None,
    },
    Rule {
        needle: "exposes no ValuePattern",
        from: UIA,
        apps: &["ui"],
        advice: "That control is not a text box, so text cannot be typed into it. Find the text box itself in :tree (type=Edit), or press the control with struct verb invoke.",
        mcp: None,
    },
    Rule {
        needle: "that control is read-only",
        from: UIA,
        apps: &["ui"],
        advice: "That field cannot be changed from here. Look in :tree for an editable field (type=Edit) or a button that changes it.",
        mcp: None,
    },
    Rule {
        needle: "supports neither Invoke nor Toggle",
        from: UIA,
        apps: &["ui"],
        advice: "That control cannot be pressed. Use action=select for list items and tabs, expand or collapse for tree nodes and menus, focus to move to it. :tree shows each control's type.",
        mcp: None,
    },
    Rule {
        needle: "does not support",
        from: UIA,
        apps: &["ui"],
        advice: "That control does not take that action. Buttons take invoke, check boxes toggle, list items and tabs select, tree nodes and menus expand or collapse. :tree shows each control's type.",
        mcp: None,
    },
    Rule {
        needle: "unknown control type",
        from: UIA,
        apps: &["ui"],
        advice: "type= takes a UI Automation control type: Button, Edit, CheckBox, ComboBox, List, ListItem, Menu, MenuItem, Tab, TabItem, Tree, TreeItem, Text, Window.",
        mcp: None,
    },
    // ---- web pages -------------------------------------------------------
    Rule {
        needle: "no element matches",
        from: CDP,
        apps: &["web"],
        advice: "Nothing on the page matches that CSS selector. Pages change as they load and after every click. Read selector body to see what is there now, then use a selector that exists.",
        mcp: None,
    },
    Rule {
        needle: "unit not found",
        from: CDP,
        apps: &["web"],
        advice: "The part of the page this handle points at is gone; the page probably navigated. Read the page with selector body to see where it is now.",
        mcp: None,
    },
];

/// Advice for a live error, if it is one we know.
pub fn advice(app: &str, error: &str, caller: Caller) -> Option<&'static str> {
    let app = app_key(app);
    let hay = error.to_ascii_lowercase();
    RULES
        .iter()
        .filter(|r| r.apps.is_empty() || r.apps.contains(&app))
        .find(|r| hay.contains(&r.needle.to_ascii_lowercase()))
        .map(|r| match caller {
            Caller::Mcp => r.mcp.unwrap_or(r.advice),
            Caller::Loop => r.advice,
        })
}

/// The error with its advice after it, or the error alone.
pub fn explain(app: &str, error: &str, caller: Caller) -> String {
    match advice(app, error, caller) {
        Some(a) => format!("{error}\nWhat to do: {a}"),
        None => error.to_string(),
    }
}

/// The app a handle prefix or app name means, in the five keys used here.
pub fn app_key(app: &str) -> &str {
    match app {
        "powerpoint" | "pptx" => "ppt",
        "uia" | "window" => "ui",
        "cdp" | "browser" => "web",
        other => other,
    }
}

/// How to address a part of each app, in one line. Shown to a model beside
/// its open handles and after a bad selector, so the grammar is in front of
/// it before the first call rather than after the first mistake.
pub fn selectors(app: &str) -> &'static str {
    match app_key(app) {
        "excel" => "Excel selectors name the sheet: Sheet1!A1:D10, or 'Q3 sales'!B2 when the name has a space.",
        "word" => "Word selectors are body, or p0, p1 ... for one paragraph; p0 is the first.",
        "ppt" => "PowerPoint selectors are deck, or s1, s2 ... for one slide, s2.notes for its notes.",
        "web" => "Selectors on a web page are CSS: h1, #total, table tr:nth-child(2).",
        "ui" => "Window selectors are :tree for the control list, or id=..., name=..., type=... joined by commas.",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_errors_office_host_really_sends_come_back_with_a_way_out() {
        let cases = [
            ("excel", "live app refused the op: com 0x800A03EC: Exception from HRESULT: 0x800A03EC", "English with commas"),
            ("excel", "live app refused the op: com 0x800AC472: Exception from HRESULT: 0x800AC472", "press Esc"),
            ("excel", "live app refused the op: modal dialog or busy app: human confirm required (call cancelled after 15000ms, app untouched)", "close the dialog"),
            ("word", "live app refused the op: com 0x800A1066: Command failed", "Protected View"),
            ("excel", "live app refused the op: com 0x800A03EC: The cell or chart you're trying to change is on a protected sheet.", "Unprotect"),
            ("ppt", "live app refused the op: presentation not open for ppt:deck.pptx:deck", "open it again"),
            ("ui", "live app refused the op: no control matches name=Save", ":tree"),
            ("web", "live app refused the op: no element matches #total in :doc", "selector body"),
        ];
        for (app, err, want) in cases {
            let got = advice(app, err, Caller::Loop).unwrap_or_else(|| panic!("no advice for {err}"));
            assert!(got.contains(want), "{err}\n  -> {got}");
        }
    }

    #[test]
    fn a_protected_sheet_is_named_as_that_not_as_the_vague_excel_code() {
        // The code and the words arrive together; the words are the more
        // useful, so that rule has to win.
        let got = advice("excel", "com 0x800A03EC: ... on a protected sheet.", Caller::Loop).unwrap();
        assert!(got.contains("protected against changes"), "{got}");
    }

    #[test]
    fn behind_mcp_the_way_out_is_open_and_in_the_loop_it_is_the_user() {
        let e = "com 0x800706BA: The RPC server is unavailable.";
        assert!(advice("excel", e, Caller::Mcp).unwrap().contains("Call `open`"));
        let l = advice("excel", e, Caller::Loop).unwrap();
        assert!(!l.contains("`open`"), "the loop has no open tool: {l}");
        assert!(l.contains("Ask the user"));
    }

    #[test]
    fn an_error_from_another_app_does_not_borrow_its_advice() {
        // "no element matches" is the browser's; a window's miss says
        // something else, and an Excel code means nothing to Word.
        assert!(advice("word", "com 0x800A03EC", Caller::Loop).is_none());
        assert!(advice("excel", "no element matches x", Caller::Loop).is_none());
    }

    #[test]
    fn an_unknown_error_is_passed_through_untouched() {
        assert_eq!(explain("excel", "something new", Caller::Loop), "something new");
        let e = explain("excel", "com 0x800AC472: busy", Caller::Mcp);
        assert!(e.starts_with("com 0x800AC472: busy\nWhat to do: "), "the app's own words stay first: {e}");
    }

    #[test]
    fn advice_never_tells_a_model_to_work_around_the_person() {
        for r in RULES {
            let a = r.advice.to_ascii_lowercase();
            for bad in ["click ok", "dismiss", "close the dialog yourself", "sendkeys", "kill"] {
                assert!(!a.contains(bad), "{:?} advises {bad:?}", r.needle);
            }
        }
    }

    #[test]
    fn every_app_has_a_selector_line_and_aliases_find_it() {
        for app in ["excel", "word", "ppt", "powerpoint", "web", "cdp", "ui", "uia"] {
            assert!(!selectors(app).is_empty(), "{app}");
        }
    }
}
