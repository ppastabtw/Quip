use std::io::{self, BufRead, Write};

use quip_contracts::PredictionRequest;
use serde::{Deserialize, Serialize};

use crate::InferenceBackend;

/// Internal sidecar transport. The request and response values themselves use
/// the shared Phase 0 contracts without adding transport fields to them.
#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum SidecarCommand {
    Health {
        #[serde(default)]
        case_id: Option<String>,
    },
    Predict {
        request: PredictionRequest,
    },
}

#[derive(Debug, Serialize)]
struct ProtocolFailure {
    status: &'static str,
    error: ProtocolError,
}

#[derive(Debug, Serialize)]
struct ProtocolError {
    code: &'static str,
    message: &'static str,
    retryable: bool,
}

pub fn serve<R: BufRead, W: Write, B: InferenceBackend>(
    reader: R,
    mut writer: W,
    backend: &B,
) -> io::Result<()> {
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        match serde_json::from_str::<SidecarCommand>(&line) {
            Ok(SidecarCommand::Health { case_id }) => {
                serde_json::to_writer(&mut writer, &backend.health(case_id.as_deref()))
                    .map_err(json_error)?;
            }
            Ok(SidecarCommand::Predict { request }) => {
                serde_json::to_writer(&mut writer, &backend.predict(&request))
                    .map_err(json_error)?;
            }
            Err(_) => {
                // Do not echo the rejected line: it may contain a private draft.
                serde_json::to_writer(
                    &mut writer,
                    &ProtocolFailure {
                        status: "protocol_error",
                        error: ProtocolError {
                            code: "invalid_command",
                            message: "The sidecar command is malformed.",
                            retryable: false,
                        },
                    },
                )
                .map_err(json_error)?;
            }
        }
        writer.write_all(b"\n")?;
        writer.flush()?;
    }

    Ok(())
}

fn json_error(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use serde_json::Value;

    use crate::FixtureBackend;

    use super::serve;

    #[test]
    fn one_process_handles_health_prediction_and_malformed_input() {
        let input = concat!(
            "{\"operation\":\"health\"}\n",
            "{\"operation\":\"predict\",\"request\":{\"request_id\":\"new-id\",\"profile_id\":\"profile_default\",\"model_variant\":\"base\",\"draft\":\"cnt cm tmrw\",\"context_snippets\":[],\"personal_patterns\":[]}}\n",
            "{not-json}\n",
        );
        let backend = FixtureBackend::embedded().unwrap();
        let mut output = Vec::new();

        serve(Cursor::new(input), &mut output, &backend).unwrap();

        let responses: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(responses.len(), 3);
        assert_eq!(responses[0]["status"], "ready");
        assert_eq!(responses[1]["request_id"], "new-id");
        assert_eq!(responses[2]["status"], "protocol_error");
    }
}
