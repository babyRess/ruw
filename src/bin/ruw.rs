use std::process::ExitCode;

fn main() -> ExitCode {
    if let Err(error) = ruw::cli::run(std::env::args()) {
        eprintln!("{error}");
        return ExitCode::from(1);
    }

    ExitCode::SUCCESS
}
