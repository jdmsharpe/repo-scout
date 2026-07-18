use std::collections::HashSet;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// Upper bound on a single `git status` invocation. A repository on a dead
/// network mount or behind a wedged hook is reported as an error rather than
/// stalling the entire scan.
const STATUS_TIMEOUT: Duration = Duration::from_secs(30);
/// How often to poll whether the spawned `git` has finished.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Changes {
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub conflicted: usize,
}

impl Changes {
    pub fn any(&self) -> bool {
        self.staged + self.unstaged + self.untracked + self.conflicted > 0
    }
}

/// A multi-step Git operation that was started but not yet concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Merge,
    Rebase,
    CherryPick,
    Revert,
    Bisect,
}

impl Operation {
    pub fn label(self) -> &'static str {
        match self {
            Operation::Merge => "merge",
            Operation::Rebase => "rebase",
            Operation::CherryPick => "cherry-pick",
            Operation::Revert => "revert",
            Operation::Bisect => "bisect",
        }
    }
}

/// What the STATE column reports for a repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Clean,
    Dirty,
    InProgress(Operation),
    Error,
}

impl State {
    /// Every state a report can show, in legend order.
    pub const ALL: [State; 8] = [
        State::Clean,
        State::Dirty,
        State::InProgress(Operation::Merge),
        State::InProgress(Operation::Rebase),
        State::InProgress(Operation::CherryPick),
        State::InProgress(Operation::Revert),
        State::InProgress(Operation::Bisect),
        State::Error,
    ];

    pub fn label(self) -> &'static str {
        match self {
            State::Clean => "clean",
            State::Dirty => "dirty",
            State::InProgress(operation) => operation.label(),
            State::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub path: PathBuf,
    pub display_path: String,
    pub branch: String,
    pub upstream: Option<String>,
    pub upstream_gone: bool,
    pub ahead: usize,
    pub behind: usize,
    pub stash: usize,
    pub operation: Option<Operation>,
    pub changes: Changes,
    pub error: Option<String>,
}

impl Report {
    pub fn is_dirty(&self) -> bool {
        self.changes.any()
    }

    /// Anything worth acting on: changes, an unfinished operation, divergence
    /// from the upstream, a pruned upstream, stashed work, or an error.
    pub fn needs_attention(&self) -> bool {
        self.error.is_some()
            || self.operation.is_some()
            || self.is_dirty()
            || self.ahead > 0
            || self.behind > 0
            || self.upstream_gone
            || self.stash > 0
    }

    pub fn state(&self) -> State {
        if self.error.is_some() {
            State::Error
        } else if let Some(operation) = self.operation {
            State::InProgress(operation)
        } else if self.is_dirty() {
            State::Dirty
        } else {
            State::Clean
        }
    }
}

pub fn discover(roots: &[PathBuf], max_depth: usize) -> Result<Vec<PathBuf>, String> {
    let mut repositories = Vec::new();
    let mut seen = HashSet::new();

    for root in roots {
        let root = fs::canonicalize(root)
            .map_err(|error| format!("cannot scan '{}': {error}", root.display()))?;
        if !root.is_dir() {
            return Err(format!("'{}' is not a directory", root.display()));
        }
        walk(&root, 0, max_depth, &mut repositories, &mut seen)?;
    }

    Ok(repositories)
}

fn walk(
    directory: &Path,
    depth: usize,
    max_depth: usize,
    repositories: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
) -> Result<(), String> {
    if looks_like_worktree(directory) && seen.insert(directory.to_path_buf()) {
        repositories.push(directory.to_path_buf());
    }
    if depth >= max_depth {
        return Ok(());
    }

    let entries = fs::read_dir(directory)
        .map_err(|error| format!("cannot read '{}': {error}", directory.display()))?;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if !file_type.is_dir() || file_type.is_symlink() || should_skip(&entry.file_name()) {
            continue;
        }
        // Unreadable descendants are ignored so one protected cache does not hide other repos.
        let _ = walk(&entry.path(), depth + 1, max_depth, repositories, seen);
    }
    Ok(())
}

fn looks_like_worktree(directory: &Path) -> bool {
    let marker = directory.join(".git");
    marker.is_file() || (marker.is_dir() && marker.join("HEAD").is_file())
}

fn should_skip(name: &std::ffi::OsStr) -> bool {
    matches!(
        name.to_str(),
        Some(
            ".git"
                | ".hg"
                | ".svn"
                | ".cache"
                | ".tox"
                | ".venv"
                | "node_modules"
                | "target"
                | "vendor"
        )
    )
}

pub fn inspect_all(repositories: Vec<PathBuf>, jobs: usize, tracked_only: bool) -> Vec<Report> {
    if repositories.is_empty() {
        return Vec::new();
    }

    let repositories = Arc::new(repositories);
    let next = Arc::new(AtomicUsize::new(0));
    let worker_count = jobs.min(repositories.len());
    let (sender, receiver) = mpsc::channel();

    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            let repositories = Arc::clone(&repositories);
            let next = Arc::clone(&next);
            let sender = sender.clone();
            scope.spawn(move || {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = repositories.get(index) else {
                        break;
                    };
                    if sender.send(inspect(path, tracked_only)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        receiver.iter().collect()
    })
}

fn inspect(path: &Path, tracked_only: bool) -> Report {
    match run_status(path, tracked_only) {
        Ok(Some(output)) if output.status.success() => {
            let mut report = parse_status(&output.stdout);
            report.path = path.to_path_buf();
            report.operation = detect_operation(path);
            report
        }
        Ok(Some(output)) => {
            error_report(path, stderr_message(&output.stderr, output.status.code()))
        }
        Ok(None) => error_report(
            path,
            format!("Git status timed out after {} s", STATUS_TIMEOUT.as_secs()),
        ),
        Err(error) => error_report(path, format!("could not run Git: {error}")),
    }
}

fn error_report(path: &Path, message: String) -> Report {
    Report {
        path: path.to_path_buf(),
        display_path: String::new(),
        branch: "-".into(),
        upstream: None,
        upstream_gone: false,
        ahead: 0,
        behind: 0,
        stash: 0,
        operation: None,
        changes: Changes::default(),
        error: Some(message),
    }
}

/// Detects an in-progress operation from the repository's private git dir.
/// Ordering mirrors `git status`: a conflicted rebase also leaves
/// CHERRY_PICK_HEAD behind, so the rebase directories are checked first.
fn detect_operation(worktree: &Path) -> Option<Operation> {
    let git_dir = resolve_git_dir(worktree)?;
    if git_dir.join("rebase-merge").is_dir() || git_dir.join("rebase-apply").is_dir() {
        Some(Operation::Rebase)
    } else if git_dir.join("MERGE_HEAD").is_file() {
        Some(Operation::Merge)
    } else if git_dir.join("CHERRY_PICK_HEAD").is_file() {
        Some(Operation::CherryPick)
    } else if git_dir.join("REVERT_HEAD").is_file() {
        Some(Operation::Revert)
    } else if git_dir.join("BISECT_LOG").is_file() {
        Some(Operation::Bisect)
    } else {
        None
    }
}

fn resolve_git_dir(worktree: &Path) -> Option<PathBuf> {
    let marker = worktree.join(".git");
    let file_type = fs::metadata(&marker).ok()?.file_type();
    if file_type.is_dir() {
        return Some(marker);
    }
    // Only a regular file may be opened: a `.git` FIFO would block the read
    // forever. Linked worktrees and submodules point at their git dir through
    // a `gitdir: <path>` file. Cap the read so a hostile oversized `.git`
    // file cannot balloon memory.
    if !file_type.is_file() {
        return None;
    }
    let mut contents = String::new();
    fs::File::open(&marker)
        .ok()?
        .take(4096)
        .read_to_string(&mut contents)
        .ok()?;
    let target = contents.strip_prefix("gitdir:")?.trim();
    if target.is_empty() {
        return None;
    }
    let target = Path::new(target);
    if target.is_absolute() {
        Some(target.to_path_buf())
    } else {
        Some(worktree.join(target))
    }
}

fn run_status(path: &Path, tracked_only: bool) -> io::Result<Option<Output>> {
    run_with_timeout(status_command(path, tracked_only), STATUS_TIMEOUT)
}

/// Spawns `command`, draining its pipes on helper threads so a large status
/// cannot deadlock the wait. Both the process and pipe collection share the
/// same deadline, so descendants that inherit a pipe cannot stall the scan.
/// Returns `Ok(None)` when the timeout fires.
fn run_with_timeout(mut command: Command, timeout: Duration) -> io::Result<Option<Output>> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stdout_pipe = child.stdout.take().expect("stdout is piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr is piped");
    let (stdout_sender, stdout_receiver) = mpsc::sync_channel(1);
    let (stderr_sender, stderr_receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buffer);
        let _ = stdout_sender.send(buffer);
    });
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buffer);
        let _ = stderr_sender.send(buffer);
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                kill_and_reap(child);
                return Err(error);
            }
        }
        if Instant::now() >= deadline {
            kill_and_reap(child);
            return Ok(None);
        }
        std::thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
    };

    let Some(stdout) = receive_before(&stdout_receiver, deadline) else {
        return Ok(None);
    };
    let Some(stderr) = receive_before(&stderr_receiver, deadline) else {
        return Ok(None);
    };
    Ok(Some(Output {
        status,
        stdout,
        stderr,
    }))
}

fn receive_before(receiver: &mpsc::Receiver<Vec<u8>>, deadline: Instant) -> Option<Vec<u8>> {
    match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(buffer) => Some(buffer),
        Err(mpsc::RecvTimeoutError::Disconnected) => Some(Vec::new()),
        Err(mpsc::RecvTimeoutError::Timeout) => None,
    }
}

/// Reap outside the timed path: `wait` can itself block if the child is stuck
/// in an uninterruptible system call.
fn kill_and_reap(mut child: Child) {
    let _ = child.kill();
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

fn status_command(path: &Path, tracked_only: bool) -> Command {
    let untracked = if tracked_only { "no" } else { "normal" };
    let mut command = Command::new("git");
    command
        .arg("-c")
        .arg("color.ui=false")
        // A scanned repository's own config must not be able to run code:
        // core.fsmonitor can be set to an arbitrary command that `git status`
        // would execute. A command-line -c outranks the repo's config.
        .arg("-c")
        .arg("core.fsmonitor=false")
        .arg("-C")
        .arg(path)
        .arg("status")
        .arg("--porcelain=v2")
        .arg("--branch")
        // Stash counts ride the same porcelain stream: Git >= 2.35 emits a
        // `# stash <N>` header; 2.14-2.34 accept the flag and omit the
        // header. Older Git fails loudly (error rows), raising the floor
        // this tool already had for --porcelain=v2.
        .arg("--show-stash")
        .arg("-z")
        .arg(format!("--untracked-files={untracked}"))
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        // `git -C` changes directory but still honors an inherited GIT_DIR, so
        // clear the ambient repo env or every repo reports GIT_DIR's status.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_OBJECT_DIRECTORY");
    command
}

fn stderr_message(stderr: &[u8], code: Option<i32>) -> String {
    let message = String::from_utf8_lossy(stderr);
    let message = message.trim();
    if message.is_empty() {
        format!(
            "Git exited with status {}",
            code.map_or_else(|| "unknown".into(), |c| c.to_string())
        )
    } else {
        message.to_owned()
    }
}

pub fn parse_status(bytes: &[u8]) -> Report {
    let mut report = Report {
        path: PathBuf::new(),
        display_path: String::new(),
        branch: "-".into(),
        upstream: None,
        upstream_gone: false,
        ahead: 0,
        behind: 0,
        stash: 0,
        operation: None,
        changes: Changes::default(),
        error: None,
    };
    let mut saw_ab = false;
    let mut skip_orig_path = false;

    for raw_record in bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        // In `-z` mode a rename/copy record (`2 ...`) is followed by the
        // original path as its own NUL-terminated record. That record is
        // untrusted file-name data and must never be parsed as a header: a
        // file renamed from "# stash 99" would otherwise spoof the counts.
        if skip_orig_path {
            skip_orig_path = false;
            continue;
        }
        let record = String::from_utf8_lossy(raw_record);
        if let Some(value) = record.strip_prefix("# branch.head ") {
            report.branch = if value == "(detached)" {
                "detached".into()
            } else {
                value.into()
            };
        } else if let Some(value) = record.strip_prefix("# branch.upstream ") {
            report.upstream = Some(value.into());
        } else if let Some(value) = record.strip_prefix("# branch.ab ") {
            saw_ab = true;
            parse_ahead_behind(value, &mut report);
        } else if let Some(value) = record.strip_prefix("# stash ") {
            report.stash = value.trim().parse().unwrap_or(0);
        } else if record.starts_with("1 ") || record.starts_with("2 ") {
            skip_orig_path = record.starts_with("2 ");
            if let Some(xy) = record.split_ascii_whitespace().nth(1) {
                let mut states = xy.bytes();
                if states.next().is_some_and(|state| state != b'.') {
                    report.changes.staged += 1;
                }
                if states.next().is_some_and(|state| state != b'.') {
                    report.changes.unstaged += 1;
                }
            }
        } else if record.starts_with("u ") {
            report.changes.conflicted += 1;
        } else if record.starts_with("? ") {
            report.changes.untracked += 1;
        }
    }
    // git omits `# branch.ab` when the upstream ref no longer exists (a pruned
    // remote branch), which must not be read as "in sync".
    report.upstream_gone = report.upstream.is_some() && !saw_ab;
    report
}

fn parse_ahead_behind(value: &str, report: &mut Report) {
    for part in value.split_ascii_whitespace() {
        if let Some(ahead) = part.strip_prefix('+') {
            report.ahead = ahead.parse().unwrap_or(0);
        } else if let Some(behind) = part.strip_prefix('-') {
            report.behind = behind.parse().unwrap_or(0);
        }
    }
}

pub fn assign_display_paths(reports: &mut [Report], roots: &[PathBuf]) {
    let canonical_roots: Vec<PathBuf> = roots
        .iter()
        .filter_map(|root| fs::canonicalize(root).ok())
        .collect();
    let use_relative = canonical_roots.len() == 1;

    for report in reports {
        let path = if use_relative {
            report
                .path
                .strip_prefix(&canonical_roots[0])
                .unwrap_or(&report.path)
        } else {
            &report.path
        };
        report.display_path = if path.as_os_str().is_empty() {
            ".".into()
        } else {
            path.to_string_lossy().into_owned()
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_porcelain_v2_status() {
        let input = concat!(
            "# branch.oid abc123\0",
            "# branch.head feature/cool\0",
            "# branch.upstream origin/feature/cool\0",
            "# branch.ab +2 -3\0",
            "1 M. N... 100644 100644 100644 aaa bbb staged.txt\0",
            "1 .M N... 100644 100644 100644 aaa bbb modified.txt\0",
            "1 MM N... 100644 100644 100644 aaa bbb both.txt\0",
            "? new.txt\0",
            "u UU N... 100644 100644 100644 100644 aaa bbb ccc conflict.txt\0",
        );
        let report = parse_status(input.as_bytes());

        assert_eq!(report.branch, "feature/cool");
        assert_eq!(report.upstream.as_deref(), Some("origin/feature/cool"));
        assert!(!report.upstream_gone);
        assert_eq!((report.ahead, report.behind), (2, 3));
        assert_eq!(report.changes.staged, 2);
        assert_eq!(report.changes.unstaged, 2);
        assert_eq!(report.changes.untracked, 1);
        assert_eq!(report.changes.conflicted, 1);
    }

    #[test]
    fn status_command_disables_optional_locks() {
        use std::ffi::OsStr;

        let command = status_command(Path::new("."), false);
        let optional_locks = command
            .get_envs()
            .find(|(key, _)| *key == OsStr::new("GIT_OPTIONAL_LOCKS"))
            .and_then(|(_, value)| value);

        assert_eq!(optional_locks, Some(OsStr::new("0")));
    }

    #[test]
    fn status_command_neutralizes_repo_config_and_ambient_env() {
        use std::ffi::OsStr;

        let command = status_command(Path::new("."), false);

        let disables_fsmonitor = command
            .get_args()
            .any(|arg| arg == OsStr::new("core.fsmonitor=false"));
        assert!(disables_fsmonitor, "core.fsmonitor must be neutralized");

        for key in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
            let cleared = command
                .get_envs()
                .any(|(name, value)| name == OsStr::new(key) && value.is_none());
            assert!(cleared, "{key} must be cleared from the child environment");
        }
    }

    #[test]
    fn reports_gone_upstream_when_branch_ab_is_absent() {
        let input = concat!(
            "# branch.oid abc123\0",
            "# branch.head feature\0",
            "# branch.upstream origin/feature\0",
        );
        let report = parse_status(input.as_bytes());

        assert_eq!(report.upstream.as_deref(), Some("origin/feature"));
        assert!(report.upstream_gone);
        assert_eq!((report.ahead, report.behind), (0, 0));
    }

    #[test]
    fn parses_stash_header() {
        let input = concat!("# branch.head main\0", "# stash 3\0");
        let report = parse_status(input.as_bytes());

        assert_eq!(report.stash, 3);
        assert!(!report.is_dirty(), "stashes alone must not read as dirty");
        assert!(report.needs_attention());
    }

    #[test]
    fn rename_orig_paths_cannot_spoof_headers() {
        // `2 ` records carry the original path as a separate NUL record; a
        // hostile file name must not be read as a stash or branch header.
        let input = concat!(
            "# branch.head main\0",
            "# branch.upstream origin/main\0",
            "# branch.ab +0 -0\0",
            "2 R. N... 100644 100644 100644 aaa bbb R100 renamed.txt\0",
            "# stash 99\0",
            "2 R. N... 100644 100644 100644 aaa bbb R100 other.txt\0",
            "# branch.ab +9 -9\0",
            "? real-untracked.txt\0",
        );
        let report = parse_status(input.as_bytes());

        assert_eq!(report.stash, 0, "orig path must not spoof the stash count");
        assert_eq!(
            (report.ahead, report.behind),
            (0, 0),
            "orig path must not spoof ahead/behind"
        );
        assert_eq!(report.changes.staged, 2);
        assert_eq!(report.changes.untracked, 1);
    }

    #[test]
    fn status_command_requests_stash_counts() {
        use std::ffi::OsStr;

        let command = status_command(Path::new("."), false);
        let requests_stash = command
            .get_args()
            .any(|arg| arg == OsStr::new("--show-stash"));

        assert!(requests_stash, "--show-stash must ride the status call");
    }

    #[test]
    fn attention_flags_divergence_stash_and_operations() {
        let base = Report {
            path: PathBuf::new(),
            display_path: String::new(),
            branch: "main".into(),
            upstream: Some("origin/main".into()),
            upstream_gone: false,
            ahead: 0,
            behind: 0,
            stash: 0,
            operation: None,
            changes: Changes::default(),
            error: None,
        };

        assert!(!base.needs_attention());
        for needy in [
            Report {
                ahead: 1,
                ..base.clone()
            },
            Report {
                behind: 2,
                ..base.clone()
            },
            Report {
                upstream_gone: true,
                ..base.clone()
            },
            Report {
                stash: 1,
                ..base.clone()
            },
            Report {
                operation: Some(Operation::Rebase),
                ..base.clone()
            },
            Report {
                error: Some("boom".into()),
                ..base.clone()
            },
        ] {
            assert!(needy.needs_attention(), "{needy:?} must need attention");
        }
    }

    #[test]
    fn state_prefers_error_then_operation_over_dirty() {
        let mut report = parse_status(b"? new.txt\0");
        assert_eq!(report.state(), State::Dirty);

        report.operation = Some(Operation::Merge);
        assert_eq!(report.state(), State::InProgress(Operation::Merge));

        report.error = Some("boom".into());
        assert_eq!(report.state(), State::Error);
    }

    #[test]
    fn detects_operations_from_git_dir_markers() {
        let root = unique_temp_dir("repo-scout-operation");
        let git_dir = root.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        assert_eq!(detect_operation(&root), None);

        fs::write(git_dir.join("MERGE_HEAD"), "abc\n").unwrap();
        assert_eq!(detect_operation(&root), Some(Operation::Merge));

        // A conflicted rebase also writes CHERRY_PICK_HEAD; rebase must win.
        fs::write(git_dir.join("CHERRY_PICK_HEAD"), "abc\n").unwrap();
        fs::create_dir_all(git_dir.join("rebase-merge")).unwrap();
        assert_eq!(detect_operation(&root), Some(Operation::Rebase));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn resolves_gitfile_pointer_for_linked_worktrees() {
        let root = unique_temp_dir("repo-scout-gitfile");
        let worktree = root.join("wt");
        let git_dir = root.join("gitdirs/wt");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(git_dir.join("rebase-apply")).unwrap();
        fs::write(worktree.join(".git"), "gitdir: ../gitdirs/wt\n").unwrap();

        assert_eq!(detect_operation(&worktree), Some(Operation::Rebase));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn inspect_counts_stash_entries() {
        if !git_available() || !git_supports_porcelain_stash() {
            return;
        }
        let root = unique_temp_dir("repo-scout-stash");
        fs::create_dir_all(&root).unwrap();

        init_main_repo(&root);
        fs::write(root.join("file.txt"), "one\n").unwrap();
        git(&root, &["add", "file.txt"]);
        git(&root, &["commit", "-q", "-m", "init"]);
        fs::write(root.join("file.txt"), "two\n").unwrap();
        git(&root, &["stash", "-q"]);

        let report = inspect(&root, false);

        assert!(report.error.is_none(), "status errored: {:?}", report.error);
        assert_eq!(report.stash, 1);
        assert!(!report.is_dirty());
        assert_eq!(report.state(), State::Clean);
        assert!(report.needs_attention());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn inspect_reports_merge_in_progress() {
        if !git_available() {
            return;
        }
        let root = unique_temp_dir("repo-scout-merge");
        fs::create_dir_all(&root).unwrap();

        init_main_repo(&root);
        // A global merge.ff=only would refuse the merge before it conflicts.
        git(&root, &["config", "merge.ff", "false"]);
        fs::write(root.join("file.txt"), "base\n").unwrap();
        git(&root, &["add", "file.txt"]);
        git(&root, &["commit", "-q", "-m", "base"]);
        git(&root, &["checkout", "-q", "-b", "feature"]);
        fs::write(root.join("file.txt"), "feature\n").unwrap();
        git(&root, &["commit", "-q", "-a", "-m", "feature"]);
        git(&root, &["checkout", "-q", "main"]);
        fs::write(root.join("file.txt"), "main\n").unwrap();
        git(&root, &["commit", "-q", "-a", "-m", "main"]);
        // The conflicting merge is expected to fail and leave MERGE_HEAD.
        let merged = git_allowing_failure(&root, &["merge", "-q", "feature"]);
        assert!(!merged, "the fixture merge must conflict");

        let report = inspect(&root, false);

        assert!(report.error.is_none(), "status errored: {:?}", report.error);
        assert_eq!(report.state(), State::InProgress(Operation::Merge));
        assert_eq!(report.changes.conflicted, 1);
        assert!(report.needs_attention());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn run_with_timeout_kills_a_slow_child() {
        let mut command = Command::new("sleep");
        command.arg("30");

        let started = Instant::now();
        let result = run_with_timeout(command, Duration::from_millis(200)).unwrap();

        assert!(
            result.is_none(),
            "a child exceeding the timeout yields None"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the timeout must not wait for the child to finish"
        );
    }

    #[test]
    fn run_with_timeout_captures_output_of_a_fast_child() {
        let mut command = Command::new("printf");
        command.arg("hello");

        let output = run_with_timeout(command, Duration::from_secs(5))
            .unwrap()
            .expect("a fast child completes before the timeout");

        assert!(output.status.success());
        assert_eq!(output.stdout, b"hello");
    }

    #[test]
    fn run_with_timeout_does_not_wait_for_descendant_held_pipes() {
        const HELPER_MODE: &str = "REPO_SCOUT_TIMEOUT_HELPER";

        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "git::tests::timeout_descendant_helper",
                "--nocapture",
            ])
            .env(HELPER_MODE, "spawn");

        let started = Instant::now();
        let result = run_with_timeout(command, Duration::from_millis(300)).unwrap();

        assert!(result.is_none(), "inherited pipes must obey the deadline");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "pipe collection must not wait for the descendant"
        );
    }

    #[test]
    fn timeout_descendant_helper() {
        const HELPER_MODE: &str = "REPO_SCOUT_TIMEOUT_HELPER";

        match std::env::var(HELPER_MODE).as_deref() {
            Ok("spawn") => {
                let mut child = Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "git::tests::timeout_descendant_helper",
                        "--nocapture",
                    ])
                    .env(HELPER_MODE, "sleep")
                    .spawn()
                    .unwrap();
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Ok("sleep") => std::thread::sleep(Duration::from_secs(2)),
            _ => {}
        }
    }

    #[test]
    fn inspect_ignores_hostile_fsmonitor_config() {
        if !git_available() {
            return;
        }
        let root = unique_temp_dir("repo-scout-fsmonitor");
        fs::create_dir_all(&root).unwrap();
        let sentinel = root.join("PWNED");

        git(&root, &["init", "-q"]);
        let payload = format!("touch {}; false", sentinel.display());
        git(&root, &["config", "core.fsmonitor", &payload]);

        let report = inspect(&root, false);

        assert!(
            !sentinel.exists(),
            "a scanned repo's core.fsmonitor command must never run"
        );
        assert!(report.error.is_none(), "status errored: {:?}", report.error);

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn inspect_reports_staged_and_untracked_changes() {
        if !git_available() {
            return;
        }
        let root = unique_temp_dir("repo-scout-inspect");
        fs::create_dir_all(&root).unwrap();

        git(&root, &["init", "-q"]);
        fs::write(root.join("staged.txt"), "one\n").unwrap();
        git(&root, &["add", "staged.txt"]);
        fs::write(root.join("loose.txt"), "two\n").unwrap();

        let report = inspect(&root, false);

        assert!(report.error.is_none(), "status errored: {:?}", report.error);
        assert!(report.is_dirty());
        assert_eq!(report.changes.staged, 1);
        assert_eq!(report.changes.untracked, 1);

        fs::remove_dir_all(&root).unwrap();
    }

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn git(dir: &Path, args: &[&str]) {
        assert!(git_allowing_failure(dir, args), "git {args:?} failed");
    }

    fn git_allowing_failure(dir: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    /// Fixture repos must not depend on the developer's global Git config
    /// (signing, hooks, templates, init.defaultBranch) or on a recent Git:
    /// `git init -b` needs 2.28, `git symbolic-ref` works everywhere.
    fn init_main_repo(dir: &Path) {
        git(dir, &["init", "-q"]);
        git(dir, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        git(dir, &["config", "user.email", "scout@example.invalid"]);
        git(dir, &["config", "user.name", "Repo Scout"]);
        git(dir, &["config", "commit.gpgsign", "false"]);
        git(dir, &["config", "core.hooksPath", "/dev/null"]);
    }

    fn git_supports_porcelain_stash() -> bool {
        // The `# stash` porcelain v2 header arrived in Git 2.35.
        let Ok(output) = Command::new("git").arg("--version").output() else {
            return false;
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let mut numbers = text
            .split_whitespace()
            .nth(2)
            .unwrap_or_default()
            .split('.')
            .map_while(|part| part.parse::<u32>().ok());
        let major = numbers.next().unwrap_or(0);
        let minor = numbers.next().unwrap_or(0);
        (major, minor) >= (2, 35)
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{unique}"))
    }

    #[test]
    fn discovers_nested_repositories_and_skips_target() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("repo-scout-test-{unique}"));
        let nested = root.join("projects/one/.git");
        let ignored = root.join("target/hidden/.git");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&ignored).unwrap();
        fs::write(nested.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(ignored.join("HEAD"), "ref: refs/heads/main\n").unwrap();

        let repositories = discover(std::slice::from_ref(&root), 4).unwrap();
        assert_eq!(repositories, vec![root.join("projects/one")]);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn depth_zero_checks_only_the_root() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("repo-scout-depth-test-{unique}"));
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("nested/.git")).unwrap();
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(root.join("nested/.git/HEAD"), "ref: refs/heads/main\n").unwrap();

        let repositories = discover(std::slice::from_ref(&root), 0).unwrap();
        assert_eq!(repositories, vec![root.clone()]);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ignores_empty_git_shaped_directories() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("repo-scout-fake-test-{unique}"));
        fs::create_dir_all(root.join("not-a-repo/.git")).unwrap();

        let repositories = discover(std::slice::from_ref(&root), 2).unwrap();
        assert!(repositories.is_empty());

        fs::remove_dir_all(root).unwrap();
    }
}
