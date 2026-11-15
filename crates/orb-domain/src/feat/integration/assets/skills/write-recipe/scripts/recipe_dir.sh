#!/usr/bin/env bash
set -euo pipefail

if [ "${1:-}" = "--help" ]; then
  echo "Usage: recipe_dir.sh [<folder inside the repo>]"
  echo "Prints the repo's recipe folder. Exits 1 when it holds no RECIPE.md, 2 outside a git repo."
  exit 0
fi

cd "${1:-.}"
if ! common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null); then
  echo "not inside a git repo: $PWD" >&2
  exit 2
fi

if remote=$(git remote get-url origin 2>/dev/null) && [ -n "$remote" ]; then
  name=${remote%/}
  name=${name%.git}
  name=${name##*[:/]}
else
  name=$(basename "$(dirname "$common")")
fi

folder="$HOME/.orb/recipes/$name"
echo "$folder"
[ -f "$folder/RECIPE.md" ]
