use std::process::ExitCode;

fn main() -> ExitCode {
    repo_scout::run(std::env::args_os())
}
