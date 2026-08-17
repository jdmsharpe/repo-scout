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
        .map(|report| display_width(&sanitize_text(&report.branch)))
        .max()
        .unwrap_or(6)
        .clamp(6, 28);
    let sync_width = reports
        .iter()
        .map(|report| display_width(&sync_text(report)))
        .max()
        .unwrap_or(4)
        .max(4);
    let changes_width = reports
        .iter()
        .map(|report| display_width(&changes_text(&report.changes, report.stash)))
        .max()
        .unwrap_or(7)
        .max(7);

    println!(
        "{:<state_width$}  {:<branch_width$}  {:<sync_width$}  {:<changes_width$}  REPOSITORY",
        "STATE", "BRANCH", "SYNC", "CHANGES"
    );
    for report in reports {
        let state = colored_state(report.state(), state_width, color);
        let branch = pad_display(
            &truncate(&sanitize_text(&report.branch), branch_width),
            branch_width,
        );
        let sync = pad_display(&sync_text(report), sync_width);
        let changes = pad_display(&changes_text(&report.changes, report.stash), changes_width);
        println!(
            "{state}  {branch}  {sync}  {changes}  {}",
            sanitize_text(&report.display_path)
        );
    }
}

/// Per-repository failures, on stderr so they survive `--quiet` and never
/// contaminate a redirected table.
pub fn print_errors(reports: &[Report]) {
    for report in reports {
        if let Some(error) = &report.error {
            eprintln!(
                "  {}: {}",
                sanitize_text(&report.display_path),
                sanitize_text(error)
            );
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
        State::InProgress(Operation::Am) => "a mailbox apply (git am) is in progress",
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

fn display_width(value: &str) -> usize {
    value.chars().map(char_display_width).sum()
}

fn char_display_width(character: char) -> usize {
    let code = character as u32;
    match character {
        '\u{00}'..='\u{1f}' | '\u{7f}' => 0,
        '\u{0300}'..='\u{036f}'
        | '\u{200b}'..='\u{200f}'
        | '\u{fe00}'..='\u{fe0f}'
        | '\u{fe20}'..='\u{fe2f}' => 0,
        _ if (0xe0100..=0xe01ef).contains(&code) => 0,
        _ if is_wide(code) => 2,
        _ => 1,
    }
}

fn is_wide(code: u32) -> bool {
    matches!(
        code,
        0x1100..=0x115f
            | 0x2329..=0x232a
            | 0x2e80..=0x303e
            | 0x3040..=0xa4cf
            | 0xac00..=0xd7a3
            | 0xf900..=0xfaff
            | 0xfe10..=0xfe19
            | 0xfe30..=0xfe6f
            | 0xff00..=0xff60
            | 0xffe0..=0xffe6
            | 0x1f300..=0x1faff
            | 0x20000..=0x3fffd
    )
}

fn pad_display(value: &str, width: usize) -> String {
    let used = display_width(value);
    if used >= width {
        value.into()
    } else {
        format!("{value}{}", " ".repeat(width - used))
    }
}

fn truncate(value: &str, width: usize) -> String {
    if display_width(value) <= width {
        return value.into();
    }
    if width <= 1 {
        return "…".into();
    }
    let mut result = String::new();
    let mut used = 0;
    for character in value.chars() {
        let character_width = char_display_width(character);
        if used + character_width + 1 > width {
            break;
        }
        result.push(character);
        used += character_width;
    }
    result.push('…');
    result
}

fn sanitize_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character < ' ' || character == '\u{7f}' {
                '\u{FFFD}'
            } else {
                character
            }
        })
        .collect()
}

/// Rows are only ever given new keys: consumers must ignore unknown ones, and
/// the `state` value set stays closed so an existing `select(.state != "clean")`
/// keeps selecting the same rows. Anything newly distinguishable therefore
/// arrives as its own boolean rather than as a new `state`.
pub fn print_json(reports: &[Report]) {
    println!("[");
    for (index, report) in reports.iter().enumerate() {
        let comma = if index + 1 == reports.len() { "" } else { "," };
        println!(
            "  {{\"path\":{},\"display_path\":{},\"state\":{},\"branch\":{},\"detached\":{},\"head\":{},\"upstream\":{},\"upstream_gone\":{},\"unpublished\":{},\"ahead\":{},\"behind\":{},\"stash\":{},\"operation\":{},\"worktree\":{},\"bare\":{},\"changes\":{{\"staged\":{},\"unstaged\":{},\"untracked\":{},\"conflicted\":{}}},\"needs_attention\":{},\"error\":{}}}{comma}",
            json_string(&report.path.to_string_lossy()),
            json_string(&report.display_path),
            json_string(report.state().label()),
            json_string(&report.branch),
            report.detached,
            json_optional(report.head.as_deref()),
            json_optional(report.upstream.as_deref()),
            report.upstream_gone,
            report.unpublished(),
            report.ahead,
            report.behind,
            report.stash,
            json_optional(report.operation.map(Operation::label)),
            report.worktree,
            report.bare,
            report.changes.staged,
            report.changes.unstaged,
            report.changes.untracked,
            report.changes.conflicted,
            report.needs_attention(),
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
    fn truncates_by_display_width() {
        assert_eq!(truncate("feature/very-long", 8), "feature…");
        assert_eq!(truncate("café", 4), "café");
        assert_eq!(truncate("很长的分支名", 5), "很长…");
        assert_eq!(display_width("🚀"), 2);
        assert_eq!(truncate("🚀🚀🚀", 5), "🚀🚀…");
    }

    #[test]
    fn sanitizes_control_characters_for_the_terminal() {
        assert_eq!(sanitize_text("ok"), "ok");
        assert_eq!(sanitize_text("new\nline"), "new\u{FFFD}line");
        assert_eq!(sanitize_text("\u{1b}[31mevil"), "\u{FFFD}[31mevil");
        assert_eq!(sanitize_text("bell\u{7}tab\t"), "bell\u{FFFD}tab\u{FFFD}");
    }

    fn report(upstream: Option<&str>, upstream_gone: bool, ahead: usize, behind: usize) -> Report {
        Report {
            path: std::path::PathBuf::new(),
            display_path: String::new(),
            branch: "main".into(),
            detached: false,
            head: None,
            upstream: upstream.map(String::from),
            upstream_gone,
            ahead,
            behind,
            stash: 0,
            operation: None,
            worktree: false,
            bare: false,
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
