//! texo CLI entrypoint.

use std::process::ExitCode;

#[expect(clippy::print_stderr, reason = "CLI output contract")]
fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "texo=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    if let Err(error) = batpak::event::validate_event_payload_registry() {
        eprintln!("{error}");
        return ExitCode::FAILURE;
    }

    match texo::surfaces::cli::run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error[{}]: {error}", error.code());
            ExitCode::FAILURE
        }
    }
}
