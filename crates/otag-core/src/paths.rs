//! Where otag keeps its socket, state and logs.
//!
//! Resolution is split in two: [`Dirs::from_env`] is the only function that
//! looks at the process environment, and every path rule is a pure method on
//! the resulting value. Tests build a [`Dirs`] literal instead of mutating
//! environment variables, which keeps them independent of each other and of
//! the machine they run on.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// File name of the daemon's listening socket inside its directory.
pub const SOCKET_FILE_NAME: &str = "otag.sock";

/// Environment variable that overrides the socket path.
pub const SOCKET_ENV: &str = "OTAG_SOCKET";

/// Operating-system conventions otag follows when placing its files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// `~/Library/Application Support/otag/`.
    MacOs,
    /// XDG base directories.
    Xdg,
}

impl Platform {
    /// The conventions of the system this binary was built for.
    pub const HOST: Self = if cfg!(target_os = "macos") {
        Self::MacOs
    } else {
        Self::Xdg
    };
}

/// A snapshot of the environment values otag's path rules depend on.
///
/// Construct it from the process environment with [`Dirs::from_env`], or build
/// it directly in tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    /// Value of `OTAG_SOCKET`, which overrides the socket path entirely.
    pub socket_override: Option<PathBuf>,
    /// Value of `HOME`.
    pub home: Option<PathBuf>,
    /// Value of `XDG_RUNTIME_DIR`.
    pub runtime_dir: Option<PathBuf>,
    /// Value of `XDG_STATE_HOME`.
    pub state_home: Option<PathBuf>,
    /// Conventions to apply.
    pub platform: Platform,
}

impl Dirs {
    /// Read the relevant environment variables once.
    ///
    /// Empty values are treated as unset, and relative values are ignored,
    /// matching the XDG specification.
    pub fn from_env() -> Self {
        Self {
            socket_override: env_path(SOCKET_ENV, false),
            home: env_path("HOME", true),
            runtime_dir: env_path("XDG_RUNTIME_DIR", true),
            state_home: env_path("XDG_STATE_HOME", true),
            platform: Platform::HOST,
        }
    }

    /// A [`Dirs`] with no environment values at all, for tests and defaults.
    pub fn empty(platform: Platform) -> Self {
        Self {
            socket_override: None,
            home: None,
            runtime_dir: None,
            state_home: None,
            platform,
        }
    }

    /// Path of the daemon's socket.
    ///
    /// `OTAG_SOCKET` wins if set. Otherwise the socket lives in the state
    /// directory on macOS, and in `$XDG_RUNTIME_DIR/otag/` on XDG systems,
    /// falling back to the state directory when no runtime directory exists.
    pub fn socket_path(&self) -> Result<PathBuf> {
        if let Some(path) = &self.socket_override {
            return Ok(path.clone());
        }
        let dir = match self.platform {
            Platform::MacOs => self.state_dir()?,
            Platform::Xdg => match &self.runtime_dir {
                Some(runtime) => runtime.join("otag"),
                None => self.state_dir()?,
            },
        };
        Ok(dir.join(SOCKET_FILE_NAME))
    }

    /// Directory holding otag's persistent state, such as the snapshot
    /// database.
    pub fn state_dir(&self) -> Result<PathBuf> {
        match self.platform {
            Platform::MacOs => Ok(self
                .home_dir("state")?
                .join("Library")
                .join("Application Support")
                .join("otag")),
            Platform::Xdg => match &self.state_home {
                Some(state_home) => Ok(state_home.join("otag")),
                None => Ok(self
                    .home_dir("state")?
                    .join(".local")
                    .join("state")
                    .join("otag")),
            },
        }
    }

    /// Directory holding the daemon's log files.
    pub fn log_dir(&self) -> Result<PathBuf> {
        Ok(self.state_dir()?.join("logs"))
    }

    fn home_dir(&self, what: &'static str) -> Result<&Path> {
        self.home.as_deref().ok_or_else(|| Error::Location {
            what,
            reason: "HOME is not set".to_owned(),
        })
    }
}

impl Default for Dirs {
    fn default() -> Self {
        Self::empty(Platform::HOST)
    }
}

/// Read an environment variable as a path, treating empty values as unset and
/// optionally rejecting relative paths.
fn env_path(key: &str, absolute_only: bool) -> Option<PathBuf> {
    let value: OsString = std::env::var_os(key)?;
    if value.is_empty() {
        return None;
    }
    let path = PathBuf::from(value);
    if absolute_only && !path.is_absolute() {
        return None;
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn macos() -> Dirs {
        Dirs {
            home: Some(PathBuf::from("/home/testuser")),
            ..Dirs::empty(Platform::MacOs)
        }
    }

    fn xdg() -> Dirs {
        Dirs {
            home: Some(PathBuf::from("/home/testuser")),
            ..Dirs::empty(Platform::Xdg)
        }
    }

    #[test]
    fn socket_override_wins_over_every_convention() {
        let dirs = Dirs {
            socket_override: Some(PathBuf::from("/somewhere/else/custom.sock")),
            runtime_dir: Some(PathBuf::from("/run/user/1000")),
            ..macos()
        };
        assert_eq!(
            dirs.socket_path().expect("resolves"),
            PathBuf::from("/somewhere/else/custom.sock")
        );
    }

    #[test]
    fn socket_override_does_not_need_home() {
        let dirs = Dirs {
            socket_override: Some(PathBuf::from("/tmp/otag-test/otag.sock")),
            ..Dirs::empty(Platform::Xdg)
        };
        assert_eq!(
            dirs.socket_path().expect("resolves"),
            PathBuf::from("/tmp/otag-test/otag.sock")
        );
    }

    #[test]
    fn macos_paths_follow_application_support() {
        let dirs = macos();
        assert_eq!(
            dirs.state_dir().expect("resolves"),
            PathBuf::from("/home/testuser/Library/Application Support/otag")
        );
        assert_eq!(
            dirs.socket_path().expect("resolves"),
            PathBuf::from("/home/testuser/Library/Application Support/otag/otag.sock")
        );
        assert_eq!(
            dirs.log_dir().expect("resolves"),
            PathBuf::from("/home/testuser/Library/Application Support/otag/logs")
        );
    }

    #[test]
    fn macos_ignores_the_runtime_directory() {
        let dirs = Dirs {
            runtime_dir: Some(PathBuf::from("/run/user/1000")),
            ..macos()
        };
        assert_eq!(
            dirs.socket_path().expect("resolves"),
            PathBuf::from("/home/testuser/Library/Application Support/otag/otag.sock")
        );
    }

    #[test]
    fn xdg_socket_prefers_the_runtime_directory() {
        let dirs = Dirs {
            runtime_dir: Some(PathBuf::from("/run/user/1000")),
            ..xdg()
        };
        assert_eq!(
            dirs.socket_path().expect("resolves"),
            PathBuf::from("/run/user/1000/otag/otag.sock")
        );
    }

    #[test]
    fn xdg_socket_falls_back_to_the_state_directory() {
        assert_eq!(
            xdg().socket_path().expect("resolves"),
            PathBuf::from("/home/testuser/.local/state/otag/otag.sock")
        );
    }

    #[test]
    fn xdg_state_home_is_honoured() {
        let dirs = Dirs {
            state_home: Some(PathBuf::from("/var/lib/testuser")),
            ..xdg()
        };
        assert_eq!(
            dirs.state_dir().expect("resolves"),
            PathBuf::from("/var/lib/testuser/otag")
        );
        assert_eq!(
            dirs.log_dir().expect("resolves"),
            PathBuf::from("/var/lib/testuser/otag/logs")
        );
    }

    #[test]
    fn missing_home_is_an_actionable_error() {
        let dirs = Dirs::empty(Platform::Xdg);
        let message = dirs.socket_path().expect_err("fails").to_string();
        assert!(message.contains("HOME is not set"), "message: {message}");
        assert!(message.contains(SOCKET_ENV), "message: {message}");
    }
}
