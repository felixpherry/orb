#!/usr/bin/env bash
set -uo pipefail

if [ $# -lt 1 ] || { [ "$1" != save ] && [ "$1" != verify ]; } || [ $# -gt 2 ] || { [ "$1" = verify ] && [ $# -ne 1 ]; }; then
  echo "Usage: tree_guard.sh save [<ref>] | tree_guard.sh verify" >&2
  echo "save records the code tree of <ref> (default HEAD) before a history rewrite; verify exits 1 if HEAD's code differs from it." >&2
  exit 2
fi

git_dir=$(git rev-parse --absolute-git-dir 2>/dev/null) || { echo "tree_guard.sh: not inside a git repository" >&2; exit 1; }
store="$git_dir/tree-guard"

if [ "$1" = save ]; then
  ref=${2:-HEAD}
  tree=$(git rev-parse --verify --quiet "$ref^{tree}") || { echo "tree_guard.sh: $ref does not resolve" >&2; exit 1; }
  printf '%s\n' "$tree" >"$store"
  echo "saved $tree from $ref"
  exit 0
fi

if [ ! -f "$store" ]; then
  echo "tree_guard.sh: nothing saved; run save before rewriting history" >&2
  exit 1
fi
saved=$(cat "$store")
current=$(git rev-parse 'HEAD^{tree}')
if [ "$saved" = "$current" ]; then
  echo "same code: $current"
  rm -f "$store"
  exit 0
fi
echo "the code changed: saved $saved, now $current" >&2
git diff --stat "$saved" "$current" >&2
exit 1
