use std::io;

use quip_inference_sidecar::{serve, FixtureBackend, LiveBackend};

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--live") {
        let backend = LiveBackend::local_default().unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(1);
        });
        run(&backend);
        return;
    }

    let backend = FixtureBackend::embedded().unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(1);
    });
    run(&backend);
}

fn run(backend: &impl quip_inference_sidecar::InferenceBackend) {
    if let Err(error) = serve(io::stdin().lock(), io::stdout().lock(), backend) {
        eprintln!("inference sidecar transport failed: {error}");
        std::process::exit(1);
    }
}
