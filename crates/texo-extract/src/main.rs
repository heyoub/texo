//! `texo-extract` — Stage-1 extractor binary for the `extract_via_cmd` seam.
//!
//! Invoked with a single markdown file path; reads the file, runs the
//! segment→propose→ground pipeline against the OpenRouter proposer, and writes the
//! grounded claims as newline-delimited JSON to stdout. Configuration is by
//! environment (`OPENROUTER_API_KEY`, `OPENROUTER_EXTRACTOR_MODEL`).

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use texo_core::DEFAULT_GROUNDING_THRESHOLD_PPM;
use texo_extract::{run_extraction, write_ndjson};
use texo_semantics::OpenRouterProposer;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprint!("texo-extract: {err}");
            let mut source = err.source();
            while let Some(cause) = source {
                eprint!(": {cause}");
                source = cause.source();
            }
            eprintln!();
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("usage: texo-extract <markdown-file>")?,
    );
    let source =
        std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;

    let proposer = OpenRouterProposer::new(None)?;
    let claims = run_extraction(&source, &proposer, DEFAULT_GROUNDING_THRESHOLD_PPM)?;

    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    write_ndjson(&claims, &mut lock)?;
    lock.flush()?;
    Ok(())
}
