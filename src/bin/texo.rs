//! texo CLI entrypoint.

use std::io::Write as _;
use std::process::ExitCode;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "texo=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    if let Err(error) = batpak::event::validate_event_payload_registry() {
        let _rendered = writeln!(std::io::stderr().lock(), "{error}");
        return ExitCode::FAILURE;
    }

    match texo::surfaces::cli::run() {
        Ok(code) => code,
        Err(error) => {
            let _rendered = texo::surfaces::cli::render::cli_error(&error);
            ExitCode::FAILURE
        }
    }
}
