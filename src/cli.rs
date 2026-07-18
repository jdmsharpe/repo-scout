use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{self, IsTerminal};
use std::path::PathBuf;

use crate::completions::Shell;

pub const HELP: &str = r#"repo-scout - see which Git repositories need attention

USAGE:
    repo-scout [OPTIONS] [--] [ROOT ...]

ARGS:
    [ROOT ...]             Directories to scan (default: current directory)
    --                     Treat every following argument as a ROOT

OPTIONS:
    -a, --attention        Show only repositories needing attention: changes,
                           ahead/behind or gone upstreams, stashes, operations
                           in progress, and errors
    -d, --dirty            Show only dirty repositories and errors
        --json             Emit a JSON array instead of a table
    -j, --jobs <COUNT>     Concurrent Git processes (default: CPU count, max 16)
        --max-depth <N>    Directory levels to search (default: 4)
        --tracked-only     Skip untracked files for a faster scan
        --no-color         Disable colored status labels
        --legend           Explain the table columns and states, then exit
        --completions <SHELL>
                           Print a completion script for bash, zsh, or fish
    -h, --help             Print help
    -V, --version          Print version

EXAMPLES:
    repo-scout ~/src
    repo-scout --attention ~/src
    repo-scout --dirty --tracked-only ~/src
    repo-scout --json ~/work ~/personal
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub roots: Vec<PathBuf>,
    pub attention_only: bool,
    pub dirty_only: bool,
    pub json: bool,
    pub jobs: usize,
    pub max_depth: usize,
    pub tracked_only: bool,
    no_color: bool,
}

pub enum Command {
    Run(Options),
    Help,
    Version,
    Legend { no_color: bool },
    Completions(Shell),
}

impl Options {
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, String> {
        let mut args = args.into_iter();
        let _program = args.next();
        let args: Vec<OsString> = args.collect();
        let mut roots = Vec::new();
        let mut attention_only = false;
        let mut dirty_only = false;
        let mut json = false;
        let mut jobs = default_jobs();
        let mut max_depth = 4;
        let mut tracked_only = false;
        let mut no_color = false;
        let mut legend = false;
        let mut positional_only = false;
        let mut index = 0;

        while index < args.len() {
            let argument = &args[index];
            if positional_only {
                roots.push(PathBuf::from(argument));
                index += 1;
                continue;
            }

            match argument.to_str() {
                Some("--") => positional_only = true,
                Some("-h" | "--help") => return Ok(Command::Help),
                Some("-V" | "--version") => return Ok(Command::Version),
                Some("-a" | "--attention") => attention_only = true,
                Some("-d" | "--dirty") => dirty_only = true,
                Some("--json") => json = true,
                Some("--tracked-only") => tracked_only = true,
                Some("--no-color") => no_color = true,
                // Not an early return: a later --no-color must still apply.
                Some("--legend") => legend = true,
                Some("--completions") => {
                    index += 1;
                    return Ok(Command::Completions(parse_shell(args.get(index))?));
                }
                Some(value) if value.starts_with("--completions=") => {
                    let (_, name) = value
                        .split_once('=')
                        .expect("inline options always contain '='");
                    return Ok(Command::Completions(parse_shell_name(name)?));
                }
                Some("-j" | "--jobs") => {
                    index += 1;
                    jobs = parse_number(args.get(index), "--jobs", false)?;
                }
                Some("--max-depth") => {
                    index += 1;
                    max_depth = parse_number(args.get(index), "--max-depth", true)?;
                }
                Some(value) if value.starts_with("--jobs=") => {
                    jobs = parse_inline_number(value, "--jobs", false)?;
                }
                Some(value) if value.starts_with("--max-depth=") => {
                    max_depth = parse_inline_number(value, "--max-depth", true)?;
                }
                Some(value) if value.starts_with('-') => {
                    return Err(format!("unknown option '{value}'"));
                }
                _ => roots.push(PathBuf::from(argument)),
            }
            index += 1;
        }

        if legend {
            return Ok(Command::Legend { no_color });
        }

        if roots.is_empty() {
            roots.push(PathBuf::from("."));
        }

        Ok(Command::Run(Self {
            roots,
            attention_only,
            dirty_only,
            json,
            jobs,
            max_depth,
            tracked_only,
            no_color,
        }))
    }

    pub fn color_enabled(&self) -> bool {
        !self.no_color && !self.json && stdout_colors()
    }
}

/// Whether plain stdout output should be colored (no explicit flag involved).
pub fn stdout_colors() -> bool {
    env::var_os("NO_COLOR").is_none() && io::stdout().is_terminal()
}

fn default_jobs() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(16)
}

fn parse_number(value: Option<&OsString>, option: &str, allow_zero: bool) -> Result<usize, String> {
    let Some(value) = value else {
        return Err(format!("{option} requires a value"));
    };
    parse_number_text(value.as_os_str(), option, allow_zero)
}

fn parse_inline_number(value: &str, option: &str, allow_zero: bool) -> Result<usize, String> {
    let (_, value) = value
        .split_once('=')
        .expect("inline options always contain '='");
    parse_number_text(OsStr::new(value), option, allow_zero)
}

fn parse_shell(value: Option<&OsString>) -> Result<Shell, String> {
    value
        .and_then(|value| value.to_str())
        .ok_or_else(|| "--completions requires a shell (bash, zsh, or fish)".to_owned())
        .and_then(parse_shell_name)
}

fn parse_shell_name(name: &str) -> Result<Shell, String> {
    Shell::from_name(name)
        .ok_or_else(|| format!("unsupported shell '{name}' (expected bash, zsh, or fish)"))
}

fn parse_number_text(value: &OsStr, option: &str, allow_zero: bool) -> Result<usize, String> {
    let Some(value) = value.to_str() else {
        return Err(format!("{option} must be a number"));
    };
    let number = value
        .parse::<usize>()
        .map_err(|_| format!("invalid value '{value}' for {option}"))?;
    if !allow_zero && number == 0 {
        return Err(format!("{option} must be greater than zero"));
    }
    Ok(number)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<Command, String> {
        Options::parse(arguments.iter().map(OsString::from))
    }

    #[test]
    fn defaults_to_current_directory() {
        let Command::Run(options) = parse(&["repo-scout"]).unwrap() else {
            panic!("expected run command");
        };
        assert_eq!(options.roots, vec![PathBuf::from(".")]);
        assert_eq!(options.max_depth, 4);
        assert!(!options.dirty_only);
    }

    #[test]
    fn parses_flags_and_multiple_roots() {
        let Command::Run(options) = parse(&[
            "repo-scout",
            "--dirty",
            "--jobs=3",
            "--max-depth",
            "7",
            "--tracked-only",
            "one",
            "two",
        ])
        .unwrap() else {
            panic!("expected run command");
        };
        assert_eq!(
            options.roots,
            vec![PathBuf::from("one"), PathBuf::from("two")]
        );
        assert_eq!(options.jobs, 3);
        assert_eq!(options.max_depth, 7);
        assert!(options.dirty_only);
        assert!(options.tracked_only);
    }

    #[test]
    fn rejects_zero_workers() {
        assert!(parse(&["repo-scout", "--jobs", "0"]).is_err());
    }

    #[test]
    fn parses_attention_flag() {
        let Command::Run(options) = parse(&["repo-scout", "-a"]).unwrap() else {
            panic!("expected run command");
        };
        assert!(options.attention_only);
        assert!(!options.dirty_only);
    }

    #[test]
    fn legend_flag_wins_over_a_scan() {
        assert!(matches!(
            parse(&["repo-scout", "--legend", "some-root"]),
            Ok(Command::Legend { no_color: false })
        ));
    }

    #[test]
    fn legend_honors_no_color_in_either_order() {
        assert!(matches!(
            parse(&["repo-scout", "--legend", "--no-color"]),
            Ok(Command::Legend { no_color: true })
        ));
        assert!(matches!(
            parse(&["repo-scout", "--no-color", "--legend"]),
            Ok(Command::Legend { no_color: true })
        ));
    }

    #[test]
    fn parses_completions_shell() {
        assert!(matches!(
            parse(&["repo-scout", "--completions", "zsh"]),
            Ok(Command::Completions(Shell::Zsh))
        ));
        assert!(matches!(
            parse(&["repo-scout", "--completions=fish"]),
            Ok(Command::Completions(Shell::Fish))
        ));
        assert!(parse(&["repo-scout", "--completions"]).is_err());
        assert!(parse(&["repo-scout", "--completions", "tcsh"]).is_err());
    }
}
