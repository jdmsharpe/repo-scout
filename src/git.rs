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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub path: PathBuf,
    pub display_path: String,
    pub branch: String,
    pub upstream: Option<String>,
    pub upstream_gone: bool,
    pub ahead: usize,
    pub behind: usize,
    pub changes: Changes,
    pub error: Option<String>,
}

impl Report {
    pub fn is_dirty(&self) -> bool {
        self.changes.any()
    }

    pub fn state(&self) -> &'static str {
        if self.error.is_some() {
            "error"
        } else if self.is_dirty() {
            "dirty"
        } else {
            "clean"
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
        changes: Changes::default(),
        error: Some(message),
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
        changes: Changes::default(),
        error: None,
    };
    let mut saw_ab = false;

    for raw_record in bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
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
        } else if record.starts_with("1 ") || record.starts_with("2 ") {
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
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
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
