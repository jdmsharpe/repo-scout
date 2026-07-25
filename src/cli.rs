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
    -d, --dirty            Show only dirty repositories and errors; combined
                           with --attention it selects either
        --json             Emit a JSON array instead of a table
    -j, --jobs <COUNT>     Concurrent Git processes (default: CPU count, up to
                           16; 0 selects the default)
        --max-depth <N>    Directory levels to search below each ROOT
                           (default: 4; 0 checks each ROOT itself only)
    -u, --unrestricted     Also descend into node_modules, target, vendor and
                           the other skipped directories
        --tracked-only     Skip untracked files for a faster scan; a repository
                           whose only change is untracked files then counts as
                           clean
        --color <WHEN>     Color the STATE column: auto (default), always,
                           never
        --no-color         Alias for --color never
    -q, --quiet            Print nothing on stdout; report by exit code
                           (implies --exit-code)
        --exit-code        Exit 3 when a shown repository needs attention
        --legend           Explain the table columns and states, then exit
        --completions <SHELL>
                           Print a completion script for bash, zsh, or fish
    -h, --help             Print help
    -V, --version          Print version

EXIT CODES:
    0                      nothing to report
    1                      a repository could not be inspected
    2                      usage error, or a ROOT that cannot be read
    3                      a shown repository needs attention (--exit-code)

ENVIRONMENT:
    NO_COLOR               Never color output; --color always overrides it

EXAMPLES:
    repo-scout ~/src
    repo-scout --attention ~/src
    repo-scout --dirty --tracked-only ~/src
    repo-scout -q --exit-code -a ~/src || echo 'work is waiting'
    repo-scout --json ~/work ~/personal | jq '.[] | select(.needs_attention)'
"#;

/// When to color the STATE column. `Auto` defers to the terminal and
/// `NO_COLOR`; `Always` overrides both so color survives a pipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorWhen {
    #[default]
    Auto,
    Always,
    Never,
}

impl ColorWhen {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "auto" => Some(Self::Auto),
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            _ => None,
        }
    }

    /// Whether plain stdout output should carry escape sequences.
    pub fn enabled(self) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Auto => env::var_os("NO_COLOR").is_none() && io::stdout().is_terminal(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub roots: Vec<PathBuf>,
    pub attention_only: bool,
    pub dirty_only: bool,
    pub json: bool,
    pub jobs: usize,
    pub max_depth: usize,
    pub unrestricted: bool,
    pub tracked_only: bool,
    pub quiet: bool,
    pub exit_code: bool,
    color: ColorWhen,
}

#[derive(Debug)]
pub enum Command {
    Run(Options),
    Help,
    Version,
    Legend { color: ColorWhen },
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
        let mut jobs = 0;
        let mut max_depth = 4;
        let mut unrestricted = false;
        let mut tracked_only = false;
        let mut quiet = false;
        let mut exit_code = false;
        let mut color = ColorWhen::Auto;
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
                Some("-u" | "--unrestricted") => unrestricted = true,
                Some("--tracked-only") => tracked_only = true,
                Some("-q" | "--quiet") => quiet = true,
                Some("--exit-code") => exit_code = true,
                Some("--no-color") => color = ColorWhen::Never,
                Some("--color") => {
                    index += 1;
                    color = parse_color(args.get(index))?;
                }
                Some(value) if value.starts_with("--color=") => {
                    let (_, name) = value
                        .split_once('=')
                        .expect("inline options always contain '='");
                    color = parse_color_name(name)?;
                }
                // Not an early return: a later --color must still apply.
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
                    jobs = parse_number(args.get(index), "--jobs", true)?;
                }
                Some("--max-depth") => {
                    index += 1;
                    max_depth = parse_number(args.get(index), "--max-depth", true)?;
                }
                Some(value) if value.starts_with("--jobs=") => {
                    jobs = parse_inline_number(value, "--jobs", true)?;
                }
                Some(value) if value.starts_with("--max-depth=") => {
                    max_depth = parse_inline_number(value, "--max-depth", true)?;
                }
                // Attached short value, e.g. `-j4`. Digits only, so `-json`
                // still falls through to the unknown-option arm below rather
                // than parsing as `-j` with the value "son".
                Some(value)
                    if value.len() > 2
                        && value.starts_with("-j")
                        && value[2..].bytes().all(|byte| byte.is_ascii_digit()) =>
                {
                    jobs = parse_number_text(OsStr::new(&value[2..]), "--jobs", true)?;
                }
                Some(value) if value.starts_with('-') => {
                    return Err(format!("unknown option '{value}'"));
                }
                _ => roots.push(PathBuf::from(argument)),
            }
            index += 1;
        }

        if legend {
            return Ok(Command::Legend { color });
        }

        if roots.is_empty() {
            roots.push(PathBuf::from("."));
        }

        Ok(Command::Run(Self {
            roots,
            attention_only,
            dirty_only,
            json,
            // 0 is both the "unset" sentinel and an explicit request for the
            // default, matching ripgrep's `--threads 0`.
            jobs: if jobs == 0 { default_jobs() } else { jobs },
            max_depth,
            unrestricted,
            tracked_only,
            quiet,
            // --quiet reports through the exit status, so it has to ask for
            // the attention code or it would report nothing at all.
            exit_code: exit_code || quiet,
            color,
        }))
    }

    pub fn color_enabled(&self) -> bool {
        // JSON is a machine format: `--color always` must not reach it.
        !self.json && self.color.enabled()
    }
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

fn parse_color(value: Option<&OsString>) -> Result<ColorWhen, String> {
    value
        .and_then(|value| value.to_str())
        .ok_or_else(|| "--color requires a when (auto, always, or never)".to_owned())
        .and_then(parse_color_name)
}

fn parse_color_name(name: &str) -> Result<ColorWhen, String> {
    ColorWhen::from_name(name)
        .ok_or_else(|| format!("invalid value '{name}' for --color (expected auto, always, or never)"))
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
    fn zero_workers_selects_the_default() {
        // ripgrep's `--threads 0` convention: a script computing a worker count
        // that lands on zero should get the heuristic, not a hard error.
        for arguments in [
            vec!["repo-scout", "--jobs", "0"],
            vec!["repo-scout", "--jobs=0"],
            vec!["repo-scout", "-j0"],
        ] {
            let Command::Run(options) = parse(&arguments).unwrap() else {
                panic!("expected run command for {arguments:?}");
            };
            assert_eq!(options.jobs, default_jobs(), "for {arguments:?}");
        }
    }

    #[test]
    fn parses_attached_short_job_counts_but_not_lookalikes() {
        let Command::Run(options) = parse(&["repo-scout", "-j4"]).unwrap() else {
            panic!("expected run command");
        };
        assert_eq!(options.jobs, 4);

        // Must not parse as `-j` with the value "son".
        let error = parse(&["repo-scout", "-json"]).unwrap_err();
        assert_eq!(error, "unknown option '-json'");
    }

    #[test]
    fn parses_color_when_and_no_color_alias() {
        for (arguments, expected) in [
            (vec!["repo-scout"], ColorWhen::Auto),
            (vec!["repo-scout", "--color", "always"], ColorWhen::Always),
            (vec!["repo-scout", "--color=never"], ColorWhen::Never),
            (vec!["repo-scout", "--color", "auto"], ColorWhen::Auto),
            (vec!["repo-scout", "--no-color"], ColorWhen::Never),
            // Last one wins, so --no-color can be overridden by a later --color.
            (
                vec!["repo-scout", "--no-color", "--color", "always"],
                ColorWhen::Always,
            ),
        ] {
            let Command::Run(options) = parse(&arguments).unwrap() else {
                panic!("expected run command for {arguments:?}");
            };
            assert_eq!(options.color, expected, "for {arguments:?}");
        }

        assert!(parse(&["repo-scout", "--color"]).is_err());
        let error = parse(&["repo-scout", "--color", "sometimes"]).unwrap_err();
        assert_eq!(
            error,
            "invalid value 'sometimes' for --color (expected auto, always, or never)"
        );
    }

    #[test]
    fn color_never_reaches_json_even_when_forced() {
        let Command::Run(options) = parse(&["repo-scout", "--json", "--color", "always"]).unwrap()
        else {
            panic!("expected run command");
        };
        assert!(
            !options.color_enabled(),
            "JSON is a machine format: --color always must not reach it"
        );
    }

    #[test]
    fn quiet_implies_exit_code() {
        let Command::Run(options) = parse(&["repo-scout", "-q"]).unwrap() else {
            panic!("expected run command");
        };
        assert!(options.quiet);
        assert!(
            options.exit_code,
            "--quiet reports through the exit status, so it must request the attention code"
        );

        let Command::Run(options) = parse(&["repo-scout", "--exit-code"]).unwrap() else {
            panic!("expected run command");
        };
        assert!(options.exit_code);
        assert!(!options.quiet, "--exit-code must not silence output");
    }

    #[test]
    fn parses_unrestricted() {
        for flag in ["-u", "--unrestricted"] {
            let Command::Run(options) = parse(&["repo-scout", flag]).unwrap() else {
                panic!("expected run command for {flag}");
            };
            assert!(options.unrestricted, "for {flag}");
        }
    }

    /// Every long flag advertised in HELP must actually parse. A flag can
    /// otherwise be documented, completed by all three shells, and still be
    /// rejected at runtime.
    #[test]
    fn every_documented_flag_is_accepted() {
        for flag in crate::completions::long_flags_in_help() {
            // Options that take a value get one; the rest stand alone.
            let arguments = match flag.as_str() {
                "--jobs" | "--max-depth" => vec!["repo-scout", &flag, "2"],
                "--color" => vec!["repo-scout", &flag, "never"],
                "--completions" => vec!["repo-scout", &flag, "bash"],
                _ => vec!["repo-scout", &flag],
            };
            let result = parse(&arguments);
            assert!(
                !matches!(&result, Err(message) if message.starts_with("unknown option")),
                "HELP documents {flag}, but the parser rejects it: {result:?}",
                result = result.err()
            );
        }
    }

    /// The README embeds the help output; the two drift apart silently
    /// otherwise. Compared from USAGE: onward because the README block omits
    /// HELP's title line.
    #[test]
    fn readme_embeds_the_current_help() {
        const README: &str = include_str!("../README.md");
        let body = &HELP[HELP.find("USAGE:").expect("HELP has a USAGE section")..];
        assert!(
            README.contains(body),
            "README's help block is stale - paste in the current `repo-scout --help` output"
        );
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
            Ok(Command::Legend {
                color: ColorWhen::Auto
            })
        ));
    }

    #[test]
    fn legend_honors_color_choice_in_either_order() {
        for arguments in [
            ["repo-scout", "--legend", "--no-color"],
            ["repo-scout", "--no-color", "--legend"],
            ["repo-scout", "--legend", "--color=never"],
            ["repo-scout", "--color", "never"],
        ] {
            // The last entry has no --legend, so only the first three are
            // legend commands; all four must resolve --color to Never.
            match parse(&arguments).unwrap() {
                Command::Legend { color } => assert_eq!(color, ColorWhen::Never),
                Command::Run(options) => assert_eq!(options.color, ColorWhen::Never),
                _ => panic!("expected a legend or run command"),
            }
        }
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
