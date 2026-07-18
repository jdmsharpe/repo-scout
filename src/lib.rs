mod cli;
mod git;
mod output;

use std::ffi::OsString;
use std::process::ExitCode;
use std::time::Instant;

use cli::{Command, Options};

pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let options = match Options::parse(args) {
        Ok(Command::Run(options)) => options,
        Ok(Command::Help) => {
            print!("{}", cli::HELP);
            return ExitCode::SUCCESS;
        }
        Ok(Command::Version) => {
            println!("repo-scout {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("repo-scout: {message}\n\nTry 'repo-scout --help' for more information.");
            return ExitCode::from(2);
        }
    };

    let started = Instant::now();
    let repositories = match git::discover(&options.roots, options.max_depth) {
        Ok(repositories) => repositories,
        Err(message) => {
            eprintln!("repo-scout: {message}");
            return ExitCode::from(2);
        }
    };

    let found = repositories.len();
    let mut reports = git::inspect_all(repositories, options.jobs, options.tracked_only);
    git::assign_display_paths(&mut reports, &options.roots);
    reports.sort_by(|left, right| left.display_path.cmp(&right.display_path));

    if options.dirty_only {
        reports.retain(|report| report.is_dirty() || report.error.is_some());
    }

    if options.json {
        output::print_json(&reports);
    } else {
        output::print_table(&reports, options.color_enabled());
        eprintln!(
            "Scanned {found} {} in {} ms",
            if found == 1 {
                "repository"
            } else {
                "repositories"
            },
            started.elapsed().as_millis()
        );
    }

    if reports.iter().any(|report| report.error.is_some()) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
