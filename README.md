# repo-scout

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

# Machine-readable output across multiple roots.
repo-scout --json ~/work ~/personal | jq '.[] | select(.state != "clean")'

# Shell completions (bash, zsh, or fish).
repo-scout --completions bash > ~/.local/share/bash-completion/completions/repo-scout
```

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
```

Common dependency and build directories (`node_modules`, `.venv`, `target`, and `vendor`) 
are skipped during discovery.

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
