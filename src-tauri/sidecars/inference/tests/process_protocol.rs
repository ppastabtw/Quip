use std::{
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn executable_handles_multiple_requests_over_one_process() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_quip-inference-sidecar"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{{\"operation\":\"health\"}}").unwrap();
    writeln!(
        stdin,
        "{{\"operation\":\"health\",\"case_id\":\"adapter_degraded\"}}"
    )
    .unwrap();
    writeln!(stdin, "{{\"operation\":\"predict\",\"request\":{{\"request_id\":\"process-id\",\"profile_id\":\"profile_default\",\"model_variant\":\"base\",\"draft\":\"cnt cm tmrw\",\"context_snippets\":[],\"personal_patterns\":[]}}}}").unwrap();
    drop(stdin);

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());

    let responses: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0]["status"], "ready");
    assert_eq!(responses[1]["status"], "degraded");
    assert_eq!(responses[2]["request_id"], "process-id");
    assert_eq!(responses[2]["backend"], "fixture");
}

#[test]
fn phrase_tester_compares_base_and_global_fixtures() {
    let output = Command::new(env!("CARGO_BIN_EXE_quip-phrase-tester"))
        .arg("cnt cm tmrw")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Fixture mode only — no AI model is loaded."));
    assert!(stdout.contains("Base: candidates -> I can't come tomorrow."));
    assert!(stdout.contains("Global: candidates -> Can't come tomorrow."));
}
