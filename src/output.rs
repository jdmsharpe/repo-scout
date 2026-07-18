use crate::git::{Changes, Operation, Report, State};

pub fn print_table(reports: &[Report], color: bool) {
    if reports.is_empty() {
        println!("No repositories matched.");
        return;
    }

    let state_width = reports
        .iter()
        .map(|report| report.state().label().len())
        .max()
        .unwrap_or(5)
        .max(5);
    let branch_width = reports
        .iter()
        .map(|report| char_count(&report.branch))
        .max()
        .unwrap_or(6)
        .clamp(6, 28);
    let sync_width = reports
        .iter()
        .map(|report| char_count(&sync_text(report)))
        .max()
        .unwrap_or(4)
        .max(4);
    let changes_width = reports
        .iter()
        .map(|report| char_count(&changes_text(&report.changes, report.stash)))
        .max()
        .unwrap_or(7)
        .max(7);

    println!(
        "{:<state_width$}  {:<branch_width$}  {:<sync_width$}  {:<changes_width$}  REPOSITORY",
        "STATE", "BRANCH", "SYNC", "CHANGES"
    );
    for report in reports {
        let state = colored_state(report.state(), state_width, color);
        let branch = truncate(&report.branch, branch_width);
        println!(
            "{state}  {:<branch_width$}  {:<sync_width$}  {:<changes_width$}  {}",
            branch,
            sync_text(report),
            changes_text(&report.changes, report.stash),
            report.display_path
        );
        if let Some(error) = &report.error {
            eprintln!("  {}: {error}", report.display_path);
        }
    }
}

pub fn print_legend(color: bool) {
    print!("{}", legend_string(color));
}

fn legend_string(color: bool) -> String {
    use std::fmt::Write;

    let width = State::ALL
        .iter()
        .map(|state| state.label().len())
        .max()
        .unwrap_or(5);
    let mut text = String::from("STATE\n");
    for state in State::ALL {
        writeln!(
            text,
            "  {}  {}",
            colored_state(state, width, color),
            state_description(state)
        )
        .expect("writing to String cannot fail");
    }
    text.push_str(concat!(
        "\nSYNC\n",
        "  -     no upstream configured\n",
        "  =     in sync with the upstream branch\n",
        "  \u{2191}N    N commits ahead of the upstream\n",
        "  \u{2193}N    N commits behind the upstream\n",
        "  gone  the upstream no longer exists on the remote\n",
        "\nCHANGES\n",
        "  NS  staged entries\n",
        "  NM  unstaged tracked entries\n",
        "  N?  untracked entries\n",
        "  N!  conflicted entries\n",
        "  N*  stash entries\n",
    ));
    text
}

fn state_description(state: State) -> &'static str {
    match state {
        State::Clean => "no changes, nothing in progress",
        State::Dirty => "the working tree or index has changes",
        State::InProgress(Operation::Merge) => "a merge is in progress",
        State::InProgress(Operation::Rebase) => "a rebase is in progress",
        State::InProgress(Operation::CherryPick) => "a cherry-pick is in progress",
        State::InProgress(Operation::Revert) => "a revert is in progress",
        State::InProgress(Operation::Bisect) => "a bisect is in progress",
        State::Error => "Git could not inspect the repository",
    }
}

fn colored_state(state: State, width: usize, color: bool) -> String {
    let label = state.label();
    if !color {
        return format!("{label:<width$}");
    }
    let code = match state {
        State::Clean => 32,
        State::Dirty => 33,
        State::InProgress(_) => 35,
        State::Error => 31,
    };
    // Padding lives inside the escape sequences so it does not affect table alignment.
    format!("\x1b[{code}m{label:<width$}\x1b[0m")
}

fn sync_text(report: &Report) -> String {
    if report.upstream.is_none() {
        return "-".into();
    }
    if report.upstream_gone {
        return "gone".into();
    }
    match (report.ahead, report.behind) {
        (0, 0) => "=".into(),
        (ahead, 0) => format!("↑{ahead}"),
        (0, behind) => format!("↓{behind}"),
        (ahead, behind) => format!("↑{ahead} ↓{behind}"),
    }
}

fn changes_text(changes: &Changes, stash: usize) -> String {
    let mut parts = Vec::with_capacity(5);
    if changes.staged > 0 {
        parts.push(format!("{}S", changes.staged));
    }
    if changes.unstaged > 0 {
        parts.push(format!("{}M", changes.unstaged));
    }
    if changes.untracked > 0 {
        parts.push(format!("{}?", changes.untracked));
    }
    if changes.conflicted > 0 {
        parts.push(format!("{}!", changes.conflicted));
    }
    if stash > 0 {
        parts.push(format!("{stash}*"));
    }
    if parts.is_empty() {
        return "-".into();
    }
    parts.join(" ")
}

fn char_count(value: &str) -> usize {
    value.chars().count()
}

fn truncate(value: &str, width: usize) -> String {
    if char_count(value) <= width {
        return value.into();
    }
    if width <= 1 {
        return "…".into();
    }
    let mut result: String = value.chars().take(width - 1).collect();
    result.push('…');
    result
}

pub fn print_json(reports: &[Report]) {
    println!("[");
    for (index, report) in reports.iter().enumerate() {
        let comma = if index + 1 == reports.len() { "" } else { "," };
        println!(
            "  {{\"path\":{},\"state\":{},\"branch\":{},\"upstream\":{},\"upstream_gone\":{},\"ahead\":{},\"behind\":{},\"stash\":{},\"changes\":{{\"staged\":{},\"unstaged\":{},\"untracked\":{},\"conflicted\":{}}},\"error\":{}}}{comma}",
            json_string(&report.path.to_string_lossy()),
            json_string(report.state().label()),
            json_string(&report.branch),
            json_optional(report.upstream.as_deref()),
            report.upstream_gone,
            report.ahead,
            report.behind,
            report.stash,
            report.changes.staged,
            report.changes.unstaged,
            report.changes.untracked,
            report.changes.conflicted,
            json_optional(report.error.as_deref()),
        );
    }
    println!("]");
}

fn json_optional(value: Option<&str>) -> String {
    value.map_or_else(|| "null".into(), json_string)
}

fn json_string(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character <= '\u{1f}' => {
                use std::fmt::Write;
                write!(output, "\\u{:04x}", character as u32)
                    .expect("writing to String cannot fail");
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_json_strings() {
        assert_eq!(json_string("a\n\"b\\c"), "\"a\\n\\\"b\\\\c\"");
        assert_eq!(json_string("\u{0001}"), "\"\\u0001\"");
    }

    #[test]
    fn truncates_by_characters() {
        assert_eq!(truncate("feature/very-long", 8), "feature…");
        assert_eq!(truncate("café", 4), "café");
    }

    fn report(upstream: Option<&str>, upstream_gone: bool, ahead: usize, behind: usize) -> Report {
        Report {
            path: std::path::PathBuf::new(),
            display_path: String::new(),
            branch: "main".into(),
            upstream: upstream.map(String::from),
            upstream_gone,
            ahead,
            behind,
            stash: 0,
            operation: None,
            changes: Changes::default(),
            error: None,
        }
    }

    #[test]
    fn sync_text_covers_upstream_states() {
        assert_eq!(sync_text(&report(None, false, 0, 0)), "-");
        assert_eq!(sync_text(&report(Some("origin/main"), false, 0, 0)), "=");
        assert_eq!(sync_text(&report(Some("origin/main"), false, 2, 0)), "↑2");
        assert_eq!(sync_text(&report(Some("origin/main"), false, 0, 3)), "↓3");
        assert_eq!(
            sync_text(&report(Some("origin/main"), false, 1, 4)),
            "↑1 ↓4"
        );
        assert_eq!(sync_text(&report(Some("origin/main"), true, 0, 0)), "gone");
    }

    #[test]
    fn formats_change_counts() {
        assert_eq!(changes_text(&Changes::default(), 0), "-");
        assert_eq!(
            changes_text(
                &Changes {
                    staged: 1,
                    unstaged: 2,
                    untracked: 3,
                    conflicted: 0,
                },
                0
            ),
            "1S 2M 3?"
        );
        assert_eq!(changes_text(&Changes::default(), 2), "2*");
        assert_eq!(
            changes_text(
                &Changes {
                    staged: 1,
                    unstaged: 0,
                    untracked: 0,
                    conflicted: 1,
                },
                3
            ),
            "1S 1! 3*"
        );
    }

    #[test]
    fn legend_covers_every_state_and_glyph() {
        let legend = legend_string(false);
        for state in State::ALL {
            assert!(
                legend.contains(state.label()),
                "legend misses state '{}'",
                state.label()
            );
        }
        for glyph in ["=", "gone", "\u{2191}N", "\u{2193}N", "N*", "N!"] {
            assert!(legend.contains(glyph), "legend misses glyph '{glyph}'");
        }
        assert!(
            !legend.contains('\x1b'),
            "colorless legend must have no escapes"
        );
    }

    #[test]
    fn state_column_widens_for_long_labels() {
        let clean = colored_state(State::Clean, 11, false);
        assert_eq!(clean, "clean      ");
        let pick = colored_state(State::InProgress(Operation::CherryPick), 11, false);
        assert_eq!(pick, "cherry-pick");
    }
}
