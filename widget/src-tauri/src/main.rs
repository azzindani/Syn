// Rig widget backend (Tauri shell). COMPILES ON WINDOWS ONLY (WebView2).
//! Commands below are thin wrappers over harness-core; the sidecar
//! (office-host.exe) is spawned via tauri-plugin-shell `sidecar("office-host")`
//! with `shell:allow-spawn` scoped to `binaries/office-host` + arg allowlist.
//! Job queue: tokio mpsc, one STA-affine task per open handle (= per-file mutex).
//! Events from harness-core bus are re-emitted to the widget WebView for the live feed.

#[allow(dead_code)]
fn planned_commands() -> &'static [&'static str] {
    &[
        "harness_handshake",  // (session_id) -> registry: admit/rejoin session
        "harness_attach",     // (handle, kind) -> register open file (from sidecar ROT scan)
        "harness_op",         // (session_id, handle, op, args) -> OpOut + event appended
        "harness_undo",       // (session_id, handle) -> remaining snapshots
        "harness_events",     // (session_id) -> full event stream for render
        "harness_pause",      // (session_id) -> freeze job queue, keep files open
        "harness_resume",     // (session_id) -> re-read handles, continue
        "harness_kill",       // (job_id) -> cancel + kill sidecar tree (Job Object on Win)
    ]
}

fn main() {
    // Real entrypoint (Windows): tauri::Builder with shell plugin + sidecar
    // lifecycle (spawn, health ping, backoff restart, graceful kill-tree).
    // Deferred: cannot link WebView2 in this Linux container.
    println!("harness-widget shell scaffold: build on Windows with `tauri build`");
}
