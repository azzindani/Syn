// Widget backend (Tauri shell). COMPILES ON WINDOWS ONLY (WebView2).
//! Commands below are thin wrappers over core; the sidecar
//! (office-host.exe) is spawned via tauri-plugin-shell `sidecar("office-host")`
//! with `shell:allow-spawn` scoped to `binaries/office-host` + arg allowlist.
//! Job queue: tokio mpsc, one STA-affine task per open handle (= per-file mutex).
//! Events from core bus are re-emitted to the widget WebView for the live feed.

#[allow(dead_code)]
fn planned_commands() -> &'static [&'static str] {
    &[
        "handshake",  // (session_id) -> registry: admit/rejoin session
        "attach",     // (handle, kind) -> register open file (from sidecar ROT scan)
        "op",         // (session_id, handle, op, args) -> OpOut + event appended
        "undo",       // (session_id, handle) -> remaining snapshots
        "events",     // (session_id) -> full event stream for render
        "pause",      // (session_id) -> freeze job queue, keep files open
        "resume",     // (session_id) -> re-read handles, continue
        "kill",       // (job_id) -> cancel + kill sidecar tree (Job Object on Win)
    ]
}

fn main() {
    // Real entrypoint (Windows): tauri::Builder with shell plugin + sidecar
    // lifecycle (spawn, health ping, backoff restart, graceful kill-tree).
    // Deferred: cannot link WebView2 in this Linux container.
    println!("widget shell scaffold: build on Windows with `tauri build`");
}
