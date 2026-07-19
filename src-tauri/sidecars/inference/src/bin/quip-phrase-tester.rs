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
            println!(
                "Quip phrase tester — live {} mode; inference stays on this Mac.",
                live_model_label()
            );
            let loaded = backend.health(None).loaded;
            match (loaded.base, loaded.global_adapter) {
                (true, true) => {
                    println!("Base and the global Freesolo adapter are loaded locally.")
                }
                (false, true) => println!(
                    "The global Freesolo adapter is loaded locally; Base comparison is off."
                ),
                (true, false) => println!("Only Base is loaded locally."),
                (false, false) => println!("No local model is ready."),
            }
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
        Mode::Live => println!("Live local inference — {} (Metal).", live_model_label()),
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

fn live_model_label() -> String {
    std::env::var("QUIP_GLOBAL_MODEL_ID")
        .or_else(|_| std::env::var("QUIP_BASE_MODEL_ID"))
        .unwrap_or_else(|_| "mlx-community/Qwen3.5-2B-MLX-4bit".to_owned())
        .rsplit('/')
        .next()
        .unwrap_or("Qwen3.5-2B-MLX-4bit")
        .to_owned()
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
