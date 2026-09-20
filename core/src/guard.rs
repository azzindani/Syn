//! Guard: runtime allowlist + kill switch enforced at dispatch.
//! Policy files state intent; this is the part that actually stops a job:
//! every Runner::pump passes through Guard::check first. kill() latches —
//! only a fresh Guard re-arms dispatch.

use crate::protocol::{Error, Result};

#[derive(Debug, Clone)]
pub struct Guard {
    /// None = any app allowed (REPL default). Some(list) = allowlist.
    allowed_apps: Option<Vec<String>>,
    killed: bool,
}

impl Default for Guard {
    fn default() -> Self {
        Self::open()
    }
}

impl Guard {
    pub fn open() -> Self {
        Self { allowed_apps: None, killed: false }
    }

    pub fn locked(apps: &[&str]) -> Self {
        Self { allowed_apps: Some(apps.iter().map(|s| s.to_string()).collect()), killed: false }
    }

    pub fn allowlist(apps: Vec<String>) -> Self {
        Self { allowed_apps: Some(apps), killed: false }
    }

    /// Latch the kill switch. In-flight queue drains to a stop.
    pub fn kill(&mut self) {
        self.killed = true;
    }

    pub fn is_killed(&self) -> bool {
        self.killed
    }

    /// Latch probe: call before dequeuing so a killed runner consumes nothing.
    pub fn armed(&self) -> Result<()> {
        if self.killed {
            return Err(Error::Killed);
        }
        Ok(())
    }

    /// Check one dispatch. App is the handle prefix ("excel", "word").
    pub fn check(&self, app: &str) -> Result<()> {
        if self.killed {
            return Err(Error::Killed);
        }
        if let Some(apps) = &self.allowed_apps
            && !apps.iter().any(|a| a == app)
        {
            return Err(Error::AppDenied(app.into()));
        }
        Ok(())
    }
}

/// Whether VBA may be written or run in this session.
///
/// Off unless a human turns it on, and `protocol/security_policy.json`
/// denies it by default. This is a harder gate than anything else here
/// because it is a harder capability: every other op edits a document,
/// while a macro is a program running at the user's full privilege, able
/// to reach the file system, the network and other processes. `shell`,
/// which is strictly weaker, already stops for a human on every call.
///
/// Session-scoped rather than per-call on purpose. The value of this verb
/// is the write-run-read-the-error-fix loop, and a prompt on every
/// iteration would either destroy the loop or train the human to click
/// through prompts, which is worse than no prompt at all. So the human
/// grants it once, knowingly, and the kill switch still ends the run.
///
/// Note for anyone enabling it: Office also needs "Trust access to the VBA
/// project object model" turned on. Without it `VBComponents.Add` returns
/// **null with no exception** -- verified on this machine -- so the sidecar
/// must read the module back rather than trust the add.
pub fn vba_allowed() -> bool {
    matches!(std::env::var("AGENT_VBA").as_deref(), Ok("1"))
}

/// Split "app:rest..." handles into the app prefix.
pub fn app_of(handle: &str) -> &str {
    handle.split(':').next().unwrap_or(handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_allows_all() {
        assert!(Guard::open().check("excel").is_ok());
    }

    #[test]
    fn allowlist_denies_unknown_app() {
        let g = Guard::locked(&["excel", "word"]);
        assert!(g.check("word").is_ok());
        assert!(matches!(g.check("browser"), Err(Error::AppDenied(_))));
    }

    #[test]
    fn kill_latches() {
        let mut g = Guard::open();
        g.kill();
        assert!(g.is_killed());
        assert!(matches!(g.check("excel"), Err(Error::Killed)));
    }

    #[test]
    fn app_of_splits_handles() {
        assert_eq!(app_of("excel:plan.xlsx:Sheet1"), "excel");
        assert_eq!(app_of("plain"), "plain");
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;

    #[test]
    fn empty_allowlist_denies_everything() {
        let g = Guard::locked(&[]);
        assert!(matches!(g.check("excel"), Err(Error::AppDenied(_))));
    }

    #[test]
    fn kill_wins_over_allowlist() {
        let mut g = Guard::locked(&["excel"]);
        assert!(g.check("excel").is_ok());
        g.kill();
        assert!(matches!(g.check("excel"), Err(Error::Killed)));
    }

    #[test]
    fn guard_is_clone_and_default_open() {
        let g = Guard::default();
        assert!(!g.is_killed());
        assert!(g.clone().check("anything").is_ok());
    }
}
