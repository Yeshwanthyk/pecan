//! Resolution of pi's on-disk layout and pecan's own state directory.

use std::path::{Path, PathBuf};

use crate::error::{CoreError, Result};

/// Canonical locations pecan reads from and writes to.
///
/// All paths live under pi's agent directory (`~/.pi/agent` by default).
/// Pecan never mutates anything under `sessions`, `tasks`, or `workflows`;
/// its own state lives in a sibling `pecan` directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiPaths {
    /// Pi agent directory containing everything below.
    agent_dir: PathBuf,
}

impl PiPaths {
    /// Builds paths rooted at an explicit agent directory.
    #[must_use]
    pub fn from_agent_dir(agent_dir: PathBuf) -> Self {
        Self { agent_dir }
    }

    /// Detects the agent directory from `$PECAN_AGENT_DIR` or `$HOME/.pi/agent`.
    ///
    /// # Errors
    /// Returns [`CoreError::HomeMissing`] when no home directory can be resolved.
    pub fn detect() -> Result<Self> {
        if let Ok(dir) = std::env::var("PECAN_AGENT_DIR") {
            let dir = PathBuf::from(dir);
            if dir.is_absolute() {
                return Ok(Self::from_agent_dir(dir));
            }
        }
        let home = std::env::home_dir().ok_or(CoreError::HomeMissing)?;
        Ok(Self::from_agent_dir(home.join(".pi").join("agent")))
    }

    /// The pi agent directory root.
    #[must_use]
    pub fn agent_dir(&self) -> &Path {
        &self.agent_dir
    }

    /// Directory of per-project session folders (`~/.pi/agent/sessions`).
    #[must_use]
    pub fn sessions_dir(&self) -> PathBuf {
        self.agent_dir.join("sessions")
    }

    /// Pi's global settings file, used only to detect allowlisted UI adapters.
    #[must_use]
    pub fn settings_file(&self) -> PathBuf {
        self.agent_dir.join("settings.json")
    }

    /// Directory of task-list files (`~/.pi/tasks`).
    #[must_use]
    pub fn tasks_dir(&self) -> PathBuf {
        self.agent_dir.parent().unwrap_or(self.agent_dir.as_path()).join("tasks")
    }

    /// Directory of workflow run artifacts (`~/.pi/agent/workflows`).
    #[must_use]
    pub fn workflows_dir(&self) -> PathBuf {
        self.agent_dir.join("workflows")
    }

    /// Directory pecan persists its own state into (`~/.pi/agent/pecan`).
    #[must_use]
    pub(crate) fn state_dir(&self) -> PathBuf {
        self.agent_dir.join("pecan")
    }

    /// Secret the local CLI presents to the server (`Authorization: Bearer`).
    /// Created by `pecan serve` with owner-only permissions.
    #[must_use]
    pub fn cli_token_file(&self) -> PathBuf {
        self.state_dir().join("cli-token")
    }

    /// Private key (base64url P-256 scalar) that signs this server's web push
    /// requests. Created by `pecan serve` with owner-only permissions;
    /// replacing it invalidates every browser's push subscription.
    #[must_use]
    pub fn vapid_key_file(&self) -> PathBuf {
        self.state_dir().join("vapid-key")
    }

    /// Path of pecan's `SQLite` state database.
    #[must_use]
    pub fn state_db(&self) -> PathBuf {
        self.state_dir().join("pecan.db")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_child_paths_from_agent_dir() {
        let paths = PiPaths::from_agent_dir(PathBuf::from("/tmp/fake-agent"));
        assert_eq!(paths.sessions_dir(), PathBuf::from("/tmp/fake-agent/sessions"));
        assert_eq!(paths.settings_file(), PathBuf::from("/tmp/fake-agent/settings.json"));
        assert_eq!(paths.workflows_dir(), PathBuf::from("/tmp/fake-agent/workflows"));
        assert_eq!(paths.tasks_dir(), PathBuf::from("/tmp/tasks"));
        assert_eq!(paths.state_db(), PathBuf::from("/tmp/fake-agent/pecan/pecan.db"));
        assert_eq!(paths.cli_token_file(), PathBuf::from("/tmp/fake-agent/pecan/cli-token"));
    }
}
