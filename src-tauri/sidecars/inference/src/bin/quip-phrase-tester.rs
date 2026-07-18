use std::io::{self, BufRead, Write};

use quip_contracts::{ModelVariant, PredictionRequest, PredictionResult};
use quip_inference_sidecar::{FixtureBackend, InferenceBackend, LiveBackend};

fn main() {
    let mut arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let live = arguments.first().map(String::as_str) == Some("--live");
    if live {
        arguments.remove(0);
        let backend = LiveBackend::local_default().unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(1);
        });
        run(&backend, Mode::Live, arguments);
        return;
    }

    let backend = FixtureBackend::embedded().unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(1);
    });
    run(&backend, Mode::Fixture, arguments);
}

#[derive(Clone, Copy)]
enum Mode {
    Fixture,
    Live,
}

fn run(backend: &impl InferenceBackend, mode: Mode, arguments: Vec<String>) {
    let phrase = arguments.join(" ");

    if phrase.is_empty() {
        if let Err(error) = interactive(backend, mode) {
            eprintln!("phrase tester failed: {error}");
            std::process::exit(1);
        }
    } else {
        print_comparison(backend, mode, &phrase, 1);
    }
}

fn interactive(backend: &impl InferenceBackend, mode: Mode) -> io::Result<()> {
    match mode {
        Mode::Fixture => {
            println!("Quip phrase tester — fixture mode; Qwen is not loaded yet.");
            println!("Available prerecorded phrases:");
            let fixtures = FixtureBackend::embedded().expect("embedded fixtures were already read");
            for phrase in fixtures.simple_example_drafts() {
                println!("  - {phrase}");
            }
        }
        Mode::Live => {
            println!("Quip phrase tester — live Qwen3.5-2B mode; inference stays on this Mac.");
            println!("The global Freesolo adapter is not loaded yet, so only Base can infer.");
        }
    }
    println!("Type /quit or submit a blank line to exit.");

    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let mut request_number = 1;

    loop {
        print!("\nPhrase> ");
        io::stdout().flush()?;

        let Some(line) = lines.next() else {
            break;
        };
        let phrase = line?;
        let phrase = phrase.trim();
        if phrase.is_empty() || phrase == "/quit" {
            break;
        }

        print_comparison(backend, mode, phrase, request_number);
        request_number += 1;
    }

    Ok(())
}

fn print_comparison(
    backend: &impl InferenceBackend,
    mode: Mode,
    phrase: &str,
    request_number: usize,
) {
    match mode {
        Mode::Fixture => println!("Fixture mode only — no AI model is loaded."),
        Mode::Live => println!("Live local inference — Qwen3.5-2B (4-bit, Metal)."),
    }
    for (label, variant) in [
        ("Base", ModelVariant::Base),
        ("Global", ModelVariant::Global),
    ] {
        let request = PredictionRequest {
            request_id: format!("phrase-test-{request_number}-{}", label.to_lowercase()),
            profile_id: "profile_default".to_owned(),
            model_variant: variant,
            draft: phrase.to_owned(),
            context_snippets: vec![],
            personal_patterns: vec![],
        };
        println!("{label}: {}", display_result(backend.predict(&request)));
    }
}

fn display_result(result: PredictionResult) -> String {
    match result {
        PredictionResult::Ok {
            candidates,
            backend,
            latency_ms,
            ..
        } if candidates.is_empty() => match backend {
            quip_contracts::Backend::Fixture => "skip -> no changed suggestion".to_owned(),
            quip_contracts::Backend::Live => {
                format!("skip [Live, {latency_ms} ms] -> no changed suggestion")
            }
        },
        PredictionResult::Ok {
            candidates,
            backend,
            latency_ms,
            ..
        } => match backend {
            quip_contracts::Backend::Fixture => {
                format!("candidates -> {}", candidates.join(" | "))
            }
            quip_contracts::Backend::Live => format!(
                "candidates [Live, {latency_ms} ms] -> {}",
                candidates.join(" | ")
            ),
        },
        PredictionResult::Error { error, .. } if error.code == "fixture_not_found" => {
            "unavailable -> no prerecorded fixture for this phrase".to_owned()
        }
        PredictionResult::Error { error, .. } => {
            format!("error ({}) -> {}", error.code, error.message)
        }
    }
}
