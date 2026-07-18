use std::{
    error::Error,
    fmt,
    io::{BufRead, BufReader, BufWriter, Write},
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

use quip_contracts::{
    ErrorInfo, HealthStatus, LoadedArtifacts, PredictionRequest, PredictionResult, SidecarHealth,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

#[derive(Debug)]
enum SidecarClientError {
    NotConfigured,
    Spawn(std::io::Error),
    MissingPipe(&'static str),
    Transport(std::io::Error),
    Json(serde_json::Error),
    Closed,
}

impl fmt::Display for SidecarClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConfigured => write!(formatter, "the sidecar executable was not found"),
            Self::Spawn(error) => write!(formatter, "the sidecar could not start: {error}"),
            Self::MissingPipe(name) => write!(formatter, "the sidecar {name} pipe was unavailable"),
            Self::Transport(error) => write!(formatter, "sidecar transport failed: {error}"),
            Self::Json(error) => write!(formatter, "sidecar returned invalid JSON: {error}"),
            Self::Closed => write!(formatter, "the sidecar closed its output stream"),
        }
    }
}

impl Error for SidecarClientError {}

struct SidecarProcess {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Drop for SidecarProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Persistent app-side client for the newline-delimited JSON sidecar process.
/// The child is started lazily when live health or prediction is first used.
pub struct SidecarClient {
    executable: Option<PathBuf>,
    process: Option<SidecarProcess>,
}

impl SidecarClient {
    pub fn auto() -> Self {
        #[cfg(test)]
        let executable = None;
        #[cfg(not(test))]
        let executable = resolve_executable();

        Self {
            executable,
            process: None,
        }
    }

    pub fn predict(&mut self, request: &PredictionRequest) -> PredictionResult {
        self.exchange(json!({"operation": "predict", "request": request}))
            .unwrap_or_else(|error| PredictionResult::Error {
                request_id: request.request_id.clone(),
                model_variant: request.model_variant,
                error: ErrorInfo {
                    code: "sidecar_unavailable".to_owned(),
                    message: format!("The local inference sidecar is unavailable: {error}."),
                    retryable: true,
                },
            })
    }

    pub fn health(&mut self) -> SidecarHealth {
        self.exchange(json!({"operation": "health"}))
            .unwrap_or_else(|error| SidecarHealth {
                status: HealthStatus::Unavailable,
                fixture_available: true,
                loaded: LoadedArtifacts {
                    base: false,
                    global_adapter: false,
                    user_adapter: false,
                },
                error: Some(ErrorInfo {
                    code: "sidecar_unavailable".to_owned(),
                    message: format!("The local inference sidecar is unavailable: {error}."),
                    retryable: true,
                }),
            })
    }

    fn exchange<T: DeserializeOwned>(&mut self, command: Value) -> Result<T, SidecarClientError> {
        let result = (|| {
            let process = self.ensure_process()?;
            serde_json::to_writer(&mut process.stdin, &command)
                .map_err(SidecarClientError::Json)?;
            process
                .stdin
                .write_all(b"\n")
                .map_err(SidecarClientError::Transport)?;
            process
                .stdin
                .flush()
                .map_err(SidecarClientError::Transport)?;

            let mut line = String::new();
            let bytes = process
                .stdout
                .read_line(&mut line)
                .map_err(SidecarClientError::Transport)?;
            if bytes == 0 {
                Err(SidecarClientError::Closed)
            } else {
                serde_json::from_str(&line).map_err(SidecarClientError::Json)
            }
        })();

        if result.is_err() {
            self.process = None;
        }
        result
    }

    fn ensure_process(&mut self) -> Result<&mut SidecarProcess, SidecarClientError> {
        if self.process.is_none() {
            let executable = self
                .executable
                .as_ref()
                .ok_or(SidecarClientError::NotConfigured)?;
            let mut child = Command::new(executable)
                .arg("--live")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .map_err(SidecarClientError::Spawn)?;
            let stdin = child
                .stdin
                .take()
                .ok_or(SidecarClientError::MissingPipe("stdin"))?;
            let stdout = child
                .stdout
                .take()
                .ok_or(SidecarClientError::MissingPipe("stdout"))?;
            self.process = Some(SidecarProcess {
                child,
                stdin: BufWriter::new(stdin),
                stdout: BufReader::new(stdout),
            });
        }

        Ok(self
            .process
            .as_mut()
            .expect("sidecar process was initialized"))
    }
}

#[cfg(not(test))]
fn resolve_executable() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("QUIP_INFERENCE_SIDECAR") {
        return Some(PathBuf::from(explicit));
    }

    let binary_name = if cfg!(windows) {
        std::ffi::OsString::from("quip-inference-sidecar.exe")
    } else {
        std::ffi::OsString::from("quip-inference-sidecar")
    };
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(&binary_name)));
    let development = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../target/debug")
        .join(&binary_name);

    sibling
        .into_iter()
        .chain([development])
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use quip_contracts::{ModelVariant, PredictionRequest};

    use super::SidecarClient;

    #[test]
    fn unconfigured_client_returns_explicit_contract_errors() {
        let mut client = SidecarClient::auto();
        let request = PredictionRequest {
            request_id: "missing-sidecar".to_owned(),
            profile_id: "profile_default".to_owned(),
            model_variant: ModelVariant::Base,
            draft: "cnt cm tmr".to_owned(),
            context_snippets: vec![],
            personal_patterns: vec![],
        };

        assert!(matches!(
            client.predict(&request),
            quip_contracts::PredictionResult::Error { error, .. }
                if error.code == "sidecar_unavailable"
        ));
        assert_eq!(
            client.health().status,
            quip_contracts::HealthStatus::Unavailable
        );
    }
}
