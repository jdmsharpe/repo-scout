//! Static shell completion scripts. A test walks every long flag in the help
//! text and asserts each script mentions it, so the scripts cannot silently
//! drift from the real CLI.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

impl Shell {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "bash" => Some(Self::Bash),
            "zsh" => Some(Self::Zsh),
            "fish" => Some(Self::Fish),
            _ => None,
        }
    }

    pub fn script(self) -> &'static str {
        match self {
            Self::Bash => BASH,
            Self::Zsh => ZSH,
            Self::Fish => FISH,
        }
    }
}

// No mapfile: the stock macOS bash is still 3.2.
const BASH: &str = r#"_repo_scout() {
    local cur prev word
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"
    COMPREPLY=()
    case "$prev" in
        -j|--jobs|--max-depth)
            return
            ;;
        --completions)
            while IFS= read -r word; do
                COMPREPLY+=("$word")
            done < <(compgen -W "bash zsh fish" -- "$cur")
            return
            ;;
    esac
    if [[ "$cur" == -* ]]; then
        while IFS= read -r word; do
            COMPREPLY+=("$word")
        done < <(compgen -W "-a --attention -d --dirty --json \
            -j --jobs --max-depth --tracked-only --no-color --legend \
            --completions -h --help -V --version" -- "$cur")
        return
    fi
    while IFS= read -r word; do
        COMPREPLY+=("$word")
    done < <(compgen -d -- "$cur")
}
complete -F _repo_scout repo-scout
"#;

// No `-s` for _arguments: the parser does not support stacked short options.
const ZSH: &str = r#"#compdef repo-scout
_arguments \
  '(-a --attention)'{-a,--attention}'[Show only repositories needing attention]' \
  '(-d --dirty)'{-d,--dirty}'[Show only dirty repositories and errors]' \
  '--json[Emit a JSON array instead of a table]' \
  '(-j --jobs)'{-j,--jobs}'[Concurrent Git processes]:count:' \
  '--max-depth[Directory levels to search]:depth:' \
  '--tracked-only[Skip untracked files for a faster scan]' \
  '--no-color[Disable colored status labels]' \
  '--legend[Explain the table columns and states]' \
  '--completions[Print a completion script]:shell:(bash zsh fish)' \
  '(-h --help)'{-h,--help}'[Print help]' \
  '(-V --version)'{-V,--version}'[Print version]' \
  '*:directory:_files -/'
"#;

const FISH: &str = r#"complete -c repo-scout -s a -l attention -d 'Show only repositories needing attention'
complete -c repo-scout -s d -l dirty -d 'Show only dirty repositories and errors'
complete -c repo-scout -l json -d 'Emit a JSON array instead of a table'
complete -c repo-scout -s j -l jobs -x -d 'Concurrent Git processes'
complete -c repo-scout -l max-depth -x -d 'Directory levels to search'
complete -c repo-scout -l tracked-only -d 'Skip untracked files for a faster scan'
complete -c repo-scout -l no-color -d 'Disable colored status labels'
complete -c repo-scout -l legend -d 'Explain the table columns and states'
complete -c repo-scout -l completions -x -a 'bash zsh fish' -d 'Print a completion script'
complete -c repo-scout -s h -l help -d 'Print help'
complete -c repo-scout -s V -l version -d 'Print version'
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli;

    fn long_flags_in_help() -> Vec<String> {
        let mut flags: Vec<String> = cli::HELP
            .split(|character: char| !(character.is_ascii_alphanumeric() || character == '-'))
            .filter(|token| token.starts_with("--") && token.len() > 2)
            .map(str::to_owned)
            .collect();
        flags.sort();
        flags.dedup();
        flags
    }

    #[test]
    fn scripts_cover_every_flag_in_help() {
        let flags = long_flags_in_help();
        assert!(
            flags.contains(&"--attention".to_owned()),
            "flag extraction from HELP is broken: {flags:?}"
        );

        for flag in &flags {
            assert!(
                Shell::Bash.script().contains(flag),
                "bash script misses {flag}"
            );
            assert!(
                Shell::Zsh.script().contains(flag),
                "zsh script misses {flag}"
            );
            // Fish spells long options as `-l name`.
            let fish_form = format!("-l {}", &flag[2..]);
            assert!(
                Shell::Fish.script().contains(&fish_form),
                "fish script misses {flag}"
            );
        }
    }

    #[test]
    fn resolves_shell_names() {
        assert_eq!(Shell::from_name("bash"), Some(Shell::Bash));
        assert_eq!(Shell::from_name("zsh"), Some(Shell::Zsh));
        assert_eq!(Shell::from_name("fish"), Some(Shell::Fish));
        assert_eq!(Shell::from_name("powershell"), None);
    }
}
