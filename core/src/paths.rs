//! Session paths (idle|streaming|editing) + circuit breaker.
//! Ported from word-mcp-server SessionPathMachine + SessionDirector rules:
//! streaming and editing are mutually exclusive; repeated failures trip a
//! breaker that a human must reset.

/// Mutually exclusive activity paths per session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPath {
    Idle,
    Streaming,
    Editing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    Conflict(String),
    BreakerOpen,
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict(d) => write!(f, "path conflict: {d}"),
            Self::BreakerOpen => write!(f, "circuit breaker open: human reset required"),
        }
    }
}

/// Consecutive failures before the breaker opens (word-mcp hung-threshold analogue).
pub const BREAKER_TRIPS_AT: u32 = 3;

#[derive(Debug)]
pub struct PathMachine {
    path: SessionPath,
    failures: u32,
    breaker_open: bool,
}

impl PathMachine {
    pub fn new() -> Self {
        Self { path: SessionPath::Idle, failures: 0, breaker_open: false }
    }

    pub fn path(&self) -> SessionPath {
        self.path
    }

    pub fn breaker_open(&self) -> bool {
        self.breaker_open
    }

    fn guard(&self) -> Result<(), PathError> {
        if self.breaker_open {
            return Err(PathError::BreakerOpen);
        }
        Ok(())
    }

    pub fn start_stream(&mut self) -> Result<(), PathError> {
        self.guard()?;
        if self.path == SessionPath::Editing {
            return Err(PathError::Conflict("cannot stream while editing".into()));
        }
        self.path = SessionPath::Streaming;
        Ok(())
    }

    pub fn start_edit(&mut self) -> Result<(), PathError> {
        self.guard()?;
        if self.path == SessionPath::Streaming {
            return Err(PathError::Conflict("cannot edit while streaming".into()));
        }
        self.path = SessionPath::Editing;
        Ok(())
    }

    pub fn to_idle(&mut self) {
        self.path = SessionPath::Idle;
    }

    pub fn record_success(&mut self) {
        self.failures = 0;
    }

    pub fn record_failure(&mut self) {
        self.failures += 1;
        if self.failures >= BREAKER_TRIPS_AT {
            self.breaker_open = true;
        }
    }

    pub fn reset_breaker(&mut self) {
        self.breaker_open = false;
        self.failures = 0;
        self.path = SessionPath::Idle;
    }
}

impl Default for PathMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_edit_conflict() {
        let mut m = PathMachine::new();
        m.start_stream().unwrap();
        assert!(matches!(m.start_edit(), Err(PathError::Conflict(_))));
        m.to_idle();
        m.start_edit().unwrap();
        assert!(matches!(m.start_stream(), Err(PathError::Conflict(_))));
    }

    #[test]
    fn breaker_trips_and_resets() {
        let mut m = PathMachine::new();
        m.record_failure();
        m.record_failure();
        assert!(!m.breaker_open());
        m.record_failure();
        assert!(m.breaker_open());
        assert!(matches!(m.start_stream(), Err(PathError::BreakerOpen)));
        m.reset_breaker();
        m.start_stream().unwrap();
    }

    #[test]
    fn success_clears_streak() {
        let mut m = PathMachine::new();
        m.record_failure();
        m.record_success();
        m.record_failure();
        m.record_failure();
        assert!(!m.breaker_open());
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;

    #[test]
    fn idle_roundtrip_between_modes() {
        let mut m = PathMachine::new();
        assert_eq!(m.path(), SessionPath::Idle);
        m.start_edit().unwrap();
        assert_eq!(m.path(), SessionPath::Editing);
        m.to_idle();
        m.start_stream().unwrap();
        assert_eq!(m.path(), SessionPath::Streaming);
        m.to_idle();
        assert_eq!(m.path(), SessionPath::Idle);
    }

    #[test]
    fn breaker_blocks_both_modes() {
        let mut m = PathMachine::new();
        for _ in 0..BREAKER_TRIPS_AT {
            m.record_failure();
        }
        assert!(m.breaker_open());
        assert!(matches!(m.start_stream(), Err(PathError::BreakerOpen)));
        assert!(matches!(m.start_edit(), Err(PathError::BreakerOpen)));
        m.reset_breaker();
        assert!(!m.breaker_open());
        m.start_edit().unwrap();
    }

    #[test]
    fn error_display_names_conflict() {
        let e = PathError::Conflict("cannot stream while editing".into());
        assert!(e.to_string().contains("path conflict"));
        assert!(PathError::BreakerOpen.to_string().contains("human reset"));
    }
}
