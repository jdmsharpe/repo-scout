# repo-scout

`repo-scout` scans a directory full of Git repositories and shows which ones need
attention. Repository checks run concurrently, and the release binary has no
runtime dependencies beyond Git.

```text
STATE  BRANCH  SYNC  CHANGES  REPOSITORY
clean  main    =     -        api
dirty  dev     ↑2    1S 2M 1? web
clean  main    -     -        experiments/demo
```

- `S`: staged entries
- `M`: unstaged tracked entries
- `?`: untracked entries
- `!`: conflicted entries
- `↑` / `↓`: commits ahead of / behind the upstream branch

## Build and install

```bash
cargo build --release
cargo install --path .
```

## Usage

```bash
# Scan the current directory, four levels deep.
repo-scout

# Only repositories with changes, using a cheaper tracked-files-only check.
repo-scout --dirty --tracked-only ~/src

# Machine-readable output across multiple roots.
repo-scout --json ~/work ~/personal | jq '.[] | select(.state != "clean")'
```

Run `repo-scout --help` for every option. Common dependency and build directories
(`node_modules`, `.venv`, `target`, and `vendor`) are skipped during discovery.

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
