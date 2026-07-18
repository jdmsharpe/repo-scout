#!/usr/bin/env bash

set -euo pipefail

repo_scout_runs=50
repo_scout_warmup=5
repo_scout_repositories=13
repo_scout_untracked_files=2000
repo_scout_submodules=6
repo_scout_keep_fixtures=0
repo_scout_output=""

usage() {
  cat <<'EOF'
Benchmark repo-scout against reproducible synthetic workspaces.

USAGE:
    scripts/bench.sh [OPTIONS]

OPTIONS:
        --runs <N>          Timed runs per scenario (default: 50)
        --warmup <N>        Warmup runs per scenario (default: 5)
        --repos <N>         Repositories in shallow/deep fixtures (default: 13)
        --untracked <N>     Files in the untracked-heavy fixture (default: 2000)
        --submodules <N>    Submodules in the submodule fixture (default: 6)
        --output <PATH>     Hyperfine JSON output (default: target/benchmark.json)
        --keep-fixtures     Keep the generated fixture directory
    -h, --help              Print help
EOF
}

require_value() {
  local option=$1
  local value=${2:-}
  if [[ -z "$value" ]]; then
    printf 'bench.sh: %s requires a value\n' "$option" >&2
    exit 2
  fi
}

require_positive_integer() {
  local option=$1
  local value=$2
  if [[ ! "$value" =~ ^[1-9][0-9]*$ ]]; then
    printf 'bench.sh: %s must be a positive integer\n' "$option" >&2
    exit 2
  fi
}

require_nonnegative_integer() {
  local option=$1
  local value=$2
  if [[ ! "$value" =~ ^[0-9]+$ ]]; then
    printf 'bench.sh: %s must be a nonnegative integer\n' "$option" >&2
    exit 2
  fi
}

while (($# > 0)); do
  case $1 in
    --runs)
      require_value "$1" "${2:-}"
      require_positive_integer "$1" "$2"
      repo_scout_runs=$2
      shift 2
      ;;
    --warmup)
      require_value "$1" "${2:-}"
      require_nonnegative_integer "$1" "$2"
      repo_scout_warmup=$2
      shift 2
      ;;
    --repos)
      require_value "$1" "${2:-}"
      require_positive_integer "$1" "$2"
      repo_scout_repositories=$2
      shift 2
      ;;
    --untracked)
      require_value "$1" "${2:-}"
      require_positive_integer "$1" "$2"
      repo_scout_untracked_files=$2
      shift 2
      ;;
    --submodules)
      require_value "$1" "${2:-}"
      require_positive_integer "$1" "$2"
      repo_scout_submodules=$2
      shift 2
      ;;
    --output)
      require_value "$1" "${2:-}"
      repo_scout_output=$2
      shift 2
      ;;
    --keep-fixtures)
      repo_scout_keep_fixtures=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      printf 'bench.sh: unknown option %s\n' "$1" >&2
      exit 2
      ;;
  esac
done

for repo_scout_command in cargo git hyperfine; do
  if ! command -v "$repo_scout_command" >/dev/null; then
    printf 'bench.sh: required command not found: %s\n' "$repo_scout_command" >&2
    exit 1
  fi
done

repo_scout_invocation_directory=$PWD
repo_scout_script_directory=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repo_scout_project_root=$(cd -- "$repo_scout_script_directory/.." && pwd -P)
repo_scout_binary="$repo_scout_project_root/target/release/repo-scout"

if [[ -z "$repo_scout_output" ]]; then
  repo_scout_output="$repo_scout_project_root/target/benchmark.json"
elif [[ "$repo_scout_output" != /* ]]; then
  repo_scout_output="$repo_scout_invocation_directory/$repo_scout_output"
fi

cargo build --quiet --release --manifest-path "$repo_scout_project_root/Cargo.toml"
mkdir -p -- "$(dirname -- "$repo_scout_output")"

repo_scout_fixture_root=$(mktemp -d /tmp/repo-scout-bench.XXXXXX)
cleanup() {
  if ((repo_scout_keep_fixtures)); then
    printf 'Fixtures kept at %s\n' "$repo_scout_fixture_root"
    return
  fi
  case $repo_scout_fixture_root in
    /tmp/repo-scout-bench.*)
      rm -rf -- "$repo_scout_fixture_root"
      ;;
    *)
      printf 'bench.sh: refusing to remove unexpected fixture path: %s\n' \
        "$repo_scout_fixture_root" >&2
      ;;
  esac
}
trap cleanup EXIT

repo_scout_benchmark_binary="$repo_scout_fixture_root/repo-scout"
ln -s -- "$repo_scout_binary" "$repo_scout_benchmark_binary"

create_repository() {
  local repository_path=$1
  local label=$2
  git init --quiet --initial-branch=main "$repository_path"
  printf 'fixture: %s\n' "$label" >"$repository_path/fixture.txt"
  git -C "$repository_path" add fixture.txt
  git -C "$repository_path" \
    -c user.name=repo-scout-benchmark \
    -c user.email=benchmark@example.invalid \
    commit --quiet --message='Create benchmark fixture'
}

repo_scout_shallow_root="$repo_scout_fixture_root/shallow"
repo_scout_deep_root="$repo_scout_fixture_root/deep"
repo_scout_untracked_root="$repo_scout_fixture_root/untracked"
repo_scout_submodule_root="$repo_scout_fixture_root/submodules"
mkdir -p -- \
  "$repo_scout_shallow_root" \
  "$repo_scout_deep_root" \
  "$repo_scout_untracked_root" \
  "$repo_scout_submodule_root"

for ((repo_scout_index = 1; repo_scout_index <= repo_scout_repositories; repo_scout_index++)); do
  printf -v repo_scout_suffix '%02d' "$repo_scout_index"
  create_repository \
    "$repo_scout_shallow_root/repo-$repo_scout_suffix" \
    "shallow-$repo_scout_suffix"
  create_repository \
    "$repo_scout_deep_root/team-$repo_scout_suffix/services/repo-$repo_scout_suffix" \
    "deep-$repo_scout_suffix"
done

repo_scout_untracked_repository="$repo_scout_untracked_root/repo"
create_repository "$repo_scout_untracked_repository" untracked
for ((repo_scout_index = 1; repo_scout_index <= repo_scout_untracked_files; repo_scout_index++)); do
  printf -v repo_scout_suffix '%05d' "$repo_scout_index"
  printf 'untracked fixture %s\n' "$repo_scout_suffix" \
    >"$repo_scout_untracked_repository/untracked-$repo_scout_suffix.txt"
done

repo_scout_submodule_source="$repo_scout_fixture_root/submodule-source"
repo_scout_submodule_parent="$repo_scout_submodule_root/parent"
create_repository "$repo_scout_submodule_source" submodule-source
create_repository "$repo_scout_submodule_parent" submodule-parent
for ((repo_scout_index = 1; repo_scout_index <= repo_scout_submodules; repo_scout_index++)); do
  printf -v repo_scout_suffix '%02d' "$repo_scout_index"
  git -c protocol.file.allow=always \
    -C "$repo_scout_submodule_parent" \
    submodule add --quiet \
    "$repo_scout_submodule_source" \
    "modules/module-$repo_scout_suffix"
done
git -C "$repo_scout_submodule_parent" \
  -c user.name=repo-scout-benchmark \
  -c user.email=benchmark@example.invalid \
  commit --quiet --message='Add benchmark submodules'

printf 'Fixtures: %s repos, %s untracked files, %s submodules\n' \
  "$repo_scout_repositories" \
  "$repo_scout_untracked_files" \
  "$repo_scout_submodules"

hyperfine \
  --warmup "$repo_scout_warmup" \
  --runs "$repo_scout_runs" \
  --shell=none \
  --export-json "$repo_scout_output" \
  --command-name 'shallow workspace' \
  "$repo_scout_benchmark_binary --max-depth 2 $repo_scout_shallow_root" \
  --command-name 'deep directory tree' \
  "$repo_scout_benchmark_binary --max-depth 4 $repo_scout_deep_root" \
  --command-name 'large untracked set' \
  "$repo_scout_benchmark_binary --max-depth 2 $repo_scout_untracked_root" \
  --command-name 'large untracked set (tracked only)' \
  "$repo_scout_benchmark_binary --tracked-only --max-depth 2 $repo_scout_untracked_root" \
  --command-name 'submodules' \
  "$repo_scout_benchmark_binary --max-depth 4 $repo_scout_submodule_root"

printf 'JSON results: %s\n' "$repo_scout_output"
