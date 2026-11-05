//! The client of the core engine: `pime-core.exe`, spoken to over JSON Lines.
//!
//! The core is the part of the input method that predicts, validates, resolves and scans. It is a separate process
//! (it may be shared by several front-ends), so the client starts it, writes one request line and reads one answer
//! line.
//!
//! Everything that can go wrong is a typed error, never a panic: a core that is not installed, a core that dies, a
//! core that answers with something that is not JSON - all of them end as `CORE_UNAVAILABLE` with a line that says
//! what happened.

use crate::{os_detail, AgentError, CoreEngine};
use pime_core::json::{self, Json, Limits};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::Duration;

/// The deadline of one call of the core.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(15000);

/// The environment variable that says where the core binary is.
pub const CORE_EXE_ENV: &str = "PIME_CORE_EXE";

/// The name of the core binary.
pub const CORE_EXE_NAME: &str = "pime-core.exe";

/// Where the core binary is: the environment variable `PIME_CORE_EXE` first, the file `pime-core.exe` next to this
/// binary otherwise.
pub fn core_exe_path() -> PathBuf {
    let env = std::env::var_os(CORE_EXE_ENV);
    let current = std::env::current_exe().ok();
    core_exe_path_of(env.as_deref(), current.as_deref())
}

/// The place of the core binary, as a pure function (the environment and the path of this binary are its inputs).
pub fn core_exe_path_of(env: Option<&std::ffi::OsStr>, current_exe: Option<&Path>) -> PathBuf {
    if let Some(value) = env.filter(|value| !value.is_empty()) {
        return PathBuf::from(value);
    }
    if let Some(dir) = current_exe.and_then(Path::parent) {
        return dir.join(CORE_EXE_NAME);
    }
    PathBuf::from(CORE_EXE_NAME)
}

/// The client of the core engine.
pub struct CoreClient {
    exe: PathBuf,
    timeout: Duration,
}

impl Default for CoreClient {
    fn default() -> Self {
        CoreClient::new()
    }
}

impl CoreClient {
    /// A client that starts the core where the machine says it is.
    pub fn new() -> CoreClient {
        CoreClient {
            exe: core_exe_path(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// A client that starts a core binary of its own.
    pub fn with_exe(exe: PathBuf) -> CoreClient {
        CoreClient {
            exe,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// A client with a deadline of its own.
    pub fn with_timeout(mut self, timeout: Duration) -> CoreClient {
        self.timeout = timeout;
        self
    }

    /// The binary of the core.
    pub fn exe(&self) -> &Path {
        &self.exe
    }

    /// The deadline of one call.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Sends one request line and reads one answer line.
    pub fn call_raw(&self, request: &Json) -> Result<Json, AgentError> {
        let mut line = request.to_json_string();
        line.push('\n');
        let mut child = Command::new(&self.exe)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // the core must never write into our output: the MCP server speaks JSON-RPC on it
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                AgentError::core(format!(
                    "the core ({}) could not be started: {}",
                    self.exe.display(),
                    os_detail(&error)
                ))
            })?;

        {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| AgentError::core("the core has no input"))?;
            stdin.write_all(line.as_bytes()).map_err(|error| {
                AgentError::core(format!("the request could not be sent to the core: {error}"))
            })?;
            let _ = stdin.flush();
        }
        // the input is closed here: the core answers and exits

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentError::core("the core has no output"))?;
        let (sender, receiver) = channel();
        // the answer is read on a thread of its own, so a core that never answers cannot block the deadline
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut buffer = String::new();
            let outcome = reader.read_line(&mut buffer).map(|_| buffer);
            let _ = sender.send(outcome);
        });

        let answer = match receiver.recv_timeout(self.timeout) {
            Ok(Ok(answer)) => answer,
            Ok(Err(error)) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AgentError::core(format!(
                    "the answer of the core could not be read: {error}"
                )));
            }
            Err(RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AgentError::core(format!(
                    "the core did not answer within {} ms",
                    self.timeout.as_millis()
                )));
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AgentError::core(
                    "the core stopped before it answered",
                ));
            }
        };
        let _ = child.wait();

        let answer = answer.trim_end_matches(['\r', '\n']);
        if answer.is_empty() {
            return Err(AgentError::core("the core answered with nothing"));
        }
        json::parse(answer, Limits::DEFAULT).map_err(|error| {
            AgentError::core(format!(
                "the core answered with something that is not JSON: {error}"
            ))
        })
    }
}

impl CoreEngine for CoreClient {
    fn call(&self, request: &Json) -> Result<Json, AgentError> {
        self.call_raw(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_variable_wins_over_the_binary_next_to_us() {
        let path = core_exe_path_of(
            Some(std::ffi::OsStr::new("C:\\nowhere\\core.exe")),
            Some(Path::new("C:\\tools\\pime-mcp.exe")),
        );
        assert_eq!(path, PathBuf::from("C:\\nowhere\\core.exe"));
    }

    #[test]
    fn without_the_variable_the_core_is_looked_for_next_to_us() {
        let path = core_exe_path_of(None, Some(Path::new("C:\\tools\\pime-mcp.exe")));
        assert_eq!(path, PathBuf::from("C:\\tools\\pime-core.exe"));
        // and, at the very last, in the working directory
        assert_eq!(core_exe_path_of(None, None), PathBuf::from(CORE_EXE_NAME));
    }

    #[test]
    fn a_core_that_is_not_there_is_a_clean_error() {
        let client = CoreClient::with_exe(PathBuf::from("pime-core-that-does-not-exist.exe"))
            .with_timeout(Duration::from_millis(500));
        let request = Json::obj(vec![("op", Json::str("health"))]);
        let error = client.call_raw(&request).expect_err("the core is not there");
        assert_eq!(error.code(), crate::CORE_UNAVAILABLE);
        assert!(error.message().contains("could not be started"), "{error:?}");
    }

    #[test]
    fn the_request_of_the_core_is_one_line() {
        let request = Json::obj(vec![
            ("op", Json::str("predict")),
            ("prefix", Json::str("fn ")),
        ]);
        let mut line = request.to_json_string();
        line.push('\n');
        assert_eq!(line.matches('\n').count(), 1);
        assert!(line.starts_with('{'));
    }
}
