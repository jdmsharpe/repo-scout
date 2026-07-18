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
