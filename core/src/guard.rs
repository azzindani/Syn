//! Guard: runtime allowlist + kill switch enforced at dispatch.
//! Policy files state intent; this is the part that actually stops a job:
//! every Runner::pump passes through Guard::check first. kill() latches —
//! only a fresh Guard re-arms dispatch.

use crate::protocol::{HarnessError, Result};

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
            return Err(HarnessError::Killed);
        }
        Ok(())
    }

    /// Check one dispatch. App is the handle prefix ("excel", "word").
    pub fn check(&self, app: &str) -> Result<()> {
        if self.killed {
            return Err(HarnessError::Killed);
        }
        if let Some(apps) = &self.allowed_apps
            && !apps.iter().any(|a| a == app)
        {
            return Err(HarnessError::AppDenied(app.into()));
        }
        Ok(())
    }
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
        assert!(matches!(g.check("browser"), Err(HarnessError::AppDenied(_))));
    }

    #[test]
    fn kill_latches() {
        let mut g = Guard::open();
        g.kill();
        assert!(g.is_killed());
        assert!(matches!(g.check("excel"), Err(HarnessError::Killed)));
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
        assert!(matches!(g.check("excel"), Err(HarnessError::AppDenied(_))));
    }

    #[test]
    fn kill_wins_over_allowlist() {
        let mut g = Guard::locked(&["excel"]);
        assert!(g.check("excel").is_ok());
        g.kill();
        assert!(matches!(g.check("excel"), Err(HarnessError::Killed)));
    }

    #[test]
    fn guard_is_clone_and_default_open() {
        let g = Guard::default();
        assert!(!g.is_killed());
        assert!(g.clone().check("anything").is_ok());
    }
}
