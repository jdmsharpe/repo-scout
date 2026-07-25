# repo-scout

![Hits](https://hitscounter.dev/api/hit?url=https%3A%2F%2Fgithub.com%2Fjdmsharpe%2Frepo-scout%2F&label=repo-scout&icon=github&color=%23198754&message=&style=flat&tz=UTC)
[![Version](https://img.shields.io/github/v/tag/jdmsharpe/repo-scout?sort=semver&label=version)](https://github.com/jdmsharpe/repo-scout/tags)
[![License](https://img.shields.io/github/license/jdmsharpe/repo-scout?label=license)](./LICENSE)
[![CI](https://github.com/jdmsharpe/repo-scout/actions/workflows/ci.yml/badge.svg)](https://github.com/jdmsharpe/repo-scout/actions/workflows/ci.yml)
[![Dependencies](https://deps.rs/repo/github/jdmsharpe/repo-scout/status.svg)](https://deps.rs/repo/github/jdmsharpe/repo-scout)

`repo-scout` scans a directory full of Git repositories and shows which ones need
attention. Repository checks run concurrently, and the release binary has no
runtime dependencies beyond Git.

```text
STATE  BRANCH  SYNC   CHANGES   REPOSITORY
clean  main    =      -         api
clean  main    -      -         experiments/demo
merge  main    ↑1 ↓1  1! 1*     payments
dirty  dev     ↑2     1S 2M 1?  web
```

- `S`: staged entries
- `M`: unstaged tracked entries
- `?`: untracked entries
- `!`: conflicted entries
- `*`: stash entries (counts appear with Git 2.35+; repo-scout itself needs
  Git 2.14+, where `--show-stash` was added)
- `↑` / `↓`: commits ahead of / behind the upstream branch
- `gone`: an upstream is configured but no longer exists on the remote
- STATE also surfaces operations in progress: `merge`, `rebase`, `cherry-pick`,
  `revert`, and `bisect`

Run `repo-scout --legend` for the full color-coded key.

## Build and install

Prebuilt binaries for Linux (x86_64) and macOS (Intel and Apple Silicon) are attached to each
[GitHub release](https://github.com/jdmsharpe/repo-scout/releases). Or build from source:

```bash
cargo build --release
cargo install --path .
```

## Usage

```bash
# Scan the current directory, four levels deep.
repo-scout

# Everything worth acting on: changes, ahead/behind or gone upstreams,
# stashes, operations in progress, and errors.
repo-scout --attention ~/src

# Only repositories with changes, using a cheaper tracked-files-only check.
repo-scout --dirty --tracked-only ~/src

# A shell prompt or CI gate: no output, just an exit status.
repo-scout -q -a ~/src || echo 'work is waiting'

# Check a single repository without recursing.
repo-scout --max-depth 0 .

# Keep color when piping into a pager.
repo-scout --color always -a ~/src | less -R

# Machine-readable output across multiple roots.
repo-scout --json ~/work ~/personal | jq '.[] | select(.needs_attention)'

# Branches that have never been pushed anywhere.
repo-scout --json ~/src | jq -r 'select(.unpublished) | .display_path'

# Shell completions (bash, zsh, or fish).
repo-scout --completions bash > ~/.local/share/bash-completion/completions/repo-scout
```

repo-scout only ever reads. It runs one `git status` per repository and never
fetches, checks out, or merges anything.

`repo-scout --help` output follows.

```
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
```

Common dependency and build directories (`node_modules`, `.venv`, `target`, and
`vendor`) are skipped during discovery; `--unrestricted` descends into them
anyway. A repository's own `.git` directory is never walked.

## JSON output

Each row carries the fields below. New keys are only ever **added** — consumers
should ignore ones they do not recognize — and the `state` values stay a closed
set, so an existing filter keeps selecting the same rows.

| Field | Type | Notes |
| --- | --- | --- |
| `path` | string | Absolute, canonicalized |
| `display_path` | string | Relative to the ROOT when exactly one was given; matches the table |
| `state` | string | `clean`, `dirty`, `merge`, `rebase`, `cherry-pick`, `revert`, `bisect`, `error` |
| `branch` | string | `detached` when HEAD is detached |
| `detached` | bool | Distinguishes a detached HEAD from a branch named `detached` |
| `head` | string \| null | Commit the worktree is on; `null` on an unborn branch |
| `upstream` | string \| null | Configured upstream ref |
| `upstream_gone` | bool | Upstream is configured but no longer exists on the remote |
| `unpublished` | bool | On a branch that has never been pushed |
| `ahead` / `behind` | number | Commits relative to the upstream |
| `stash` | number | Stash entries (Git 2.35+) |
| `operation` | string \| null | In-progress operation, if any |
| `worktree` | bool | A linked worktree or submodule rather than a plain checkout |
| `changes` | object | `staged`, `unstaged`, `untracked`, `conflicted` counts |
| `needs_attention` | bool | What `--attention` and `--exit-code` select on |
| `error` | string \| null | Why Git could not inspect this repository |

## Performance

An optimized build was benchmarked against 13 local repositories on WSL2 Ubuntu.
These are hot-cache results from 100 Hyperfine runs using `--shell=none` to exclude
shell startup overhead:

| Mode | Mean time |
| --- | ---: |
| Default parallel scan | 5.3 ± 0.5 ms |
| Parallel scan with `--tracked-only` | 5.1 ± 0.4 ms |
| Single worker with `--jobs 1` | 28.8 ± 0.6 ms |

The default parallel scan was about **5.7× faster** than the single-worker scan.
A separate 1–16 worker sweep found 13 workers fastest at 5.05 ± 0.22 ms; with 13
repositories, additional workers had no work to claim. Exact results will vary
with repository size, storage, Git configuration, and cache state.

Run the reproducible benchmark harness to measure shallow and deep workspaces, a
large untracked set (with and without `--tracked-only`), and submodules:

```bash
scripts/bench.sh

# A quicker run with smaller fixtures and a custom results path.
scripts/bench.sh --runs 10 --repos 6 --untracked 500 --submodules 3 \
  --output target/quick-benchmark.json
```

The harness builds the release binary, creates temporary Git fixtures under
`/tmp`, runs Hyperfine with shell execution disabled, exports detailed JSON to
`target/benchmark.json`, and removes the fixtures afterward. Run
`scripts/bench.sh --help` for all sizing and output options.
