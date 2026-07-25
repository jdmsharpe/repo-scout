mod cli;
mod completions;
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
        Ok(Command::Legend { color }) => {
            output::print_legend(color.enabled());
            return ExitCode::SUCCESS;
        }
        Ok(Command::Completions(shell)) => {
            print!("{}", shell.script());
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("repo-scout: {message}\n\nTry 'repo-scout --help' for more information.");
            return ExitCode::from(2);
        }
    };

    let started = Instant::now();
    let repositories = match git::discover(&options.roots, options.max_depth, options.unrestricted)
    {
        Ok(repositories) => repositories,
        Err(message) => {
            eprintln!("repo-scout: {message}");
            return ExitCode::from(2);
        }
    };

    // Checked only once there is something to inspect, so an empty scan still
    // costs no process spawns. Without it every repository reports its own
    // identical ENOENT.
    if !repositories.is_empty() && !git::git_available() {
        eprintln!("repo-scout: git was not found on PATH");
        return ExitCode::from(2);
    }

    let found = repositories.len();
    let mut reports = git::inspect_all(repositories, options.jobs, options.tracked_only);
    git::assign_display_paths(&mut reports, &options.roots);
    // Case-insensitive so a mixed-case tree does not read as two interleaved
    // alphabets, with a case-sensitive tiebreak to keep the order total.
    reports.sort_by(|left, right| {
        let left_key = left.display_path.to_lowercase();
        let right_key = right.display_path.to_lowercase();
        left_key
            .cmp(&right_key)
            .then_with(|| left.display_path.cmp(&right.display_path))
    });

    // Each filter selects independently: --attention --dirty shows either,
    // rather than one silently overriding the other.
    if options.attention_only || options.dirty_only {
        reports.retain(|report| {
            (options.attention_only && report.needs_attention())
                || (options.dirty_only && (report.is_dirty() || report.error.is_some()))
        });
    }

    let had_error = reports.iter().any(|report| report.error.is_some());
    let needs_attention = reports.iter().any(git::Report::needs_attention);

    if options.quiet {
        // Silence is about stdout: a hook must not be able to hide breakage.
        output::print_errors(&reports);
    } else if options.json {
        output::print_json(&reports);
    } else {
        output::print_table(&reports, options.color_enabled());
        output::print_errors(&reports);
        eprintln!(
            "Showing {} of {found} {} in {} ms",
            reports.len(),
            if found == 1 {
                "repository"
            } else {
                "repositories"
            },
            started.elapsed().as_millis()
        );
    }

    // Precedence: a scan that could not complete outranks one that merely
    // found work. Attention gets its own code so 1 and 2 keep their meanings.
    let code = if had_error {
        1
    } else if options.exit_code && needs_attention {
        3
    } else {
        0
    };
    if code != 0 && !options.quiet && !options.json {
        eprintln!("repo-scout: exit {code} - {}", exit_gloss(code));
    }
    ExitCode::from(code)
}

fn exit_gloss(code: u8) -> &'static str {
    match code {
        1 => "a repository could not be inspected",
        3 => "a shown repository needs attention",
        _ => "see 'repo-scout --help' for the exit codes",
    }
}
