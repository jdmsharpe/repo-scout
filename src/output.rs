use crate::git::{Changes, Report};

pub fn print_table(reports: &[Report], color: bool) {
    if reports.is_empty() {
        println!("No repositories matched.");
        return;
    }

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
        .map(|report| char_count(&changes_text(&report.changes)))
        .max()
        .unwrap_or(7)
        .max(7);

    println!(
        "{:<5}  {:<branch_width$}  {:<sync_width$}  {:<changes_width$}  REPOSITORY",
        "STATE", "BRANCH", "SYNC", "CHANGES"
    );
    for report in reports {
        let state = colored_state(report.state(), color);
        let branch = truncate(&report.branch, branch_width);
        println!(
            "{state}  {:<branch_width$}  {:<sync_width$}  {:<changes_width$}  {}",
            branch,
            sync_text(report),
            changes_text(&report.changes),
            report.display_path
        );
        if let Some(error) = &report.error {
            eprintln!("  {}: {error}", report.display_path);
        }
    }
}

fn colored_state(state: &str, color: bool) -> String {
    if !color {
        return format!("{state:<5}");
    }
    let code = match state {
        "clean" => 32,
        "dirty" => 33,
        _ => 31,
    };
    // Padding lives inside the escape sequences so it does not affect table alignment.
    format!("\x1b[{code}m{state:<5}\x1b[0m")
}

fn sync_text(report: &Report) -> String {
    if report.upstream.is_none() {
        return "-".into();
    }
    match (report.ahead, report.behind) {
        (0, 0) => "=".into(),
        (ahead, 0) => format!("↑{ahead}"),
        (0, behind) => format!("↓{behind}"),
        (ahead, behind) => format!("↑{ahead} ↓{behind}"),
    }
}

fn changes_text(changes: &Changes) -> String {
    if !changes.any() {
        return "-".into();
    }
    let mut parts = Vec::with_capacity(4);
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
            "  {{\"path\":{},\"state\":{},\"branch\":{},\"upstream\":{},\"ahead\":{},\"behind\":{},\"changes\":{{\"staged\":{},\"unstaged\":{},\"untracked\":{},\"conflicted\":{}}},\"error\":{}}}{comma}",
            json_string(&report.path.to_string_lossy()),
            json_string(report.state()),
            json_string(&report.branch),
            json_optional(report.upstream.as_deref()),
            report.ahead,
            report.behind,
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

    #[test]
    fn formats_change_counts() {
        assert_eq!(changes_text(&Changes::default()), "-");
        assert_eq!(
            changes_text(&Changes {
                staged: 1,
                unstaged: 2,
                untracked: 3,
                conflicted: 0,
            }),
            "1S 2M 3?"
        );
    }
}
