#!/usr/bin/env bash
set -uo pipefail

usage() {
  echo "Usage: start.sh --name <short name> --note <absolute path> [--worktree] [--project <id>] [--model <model>] [--effort <level>] [--pi]" >&2
  echo "Creates an orb session, starts the agent in it, sends the note as its first prompt, and prints session=<id> pane=<id>." >&2
  exit 2
}

name="" note="" worktree="" project="" model="" effort="" kind=claude
while [ $# -gt 0 ]; do
  case "$1" in
    --name) name=${2:-}; shift 2 ;;
    --note) note=${2:-}; shift 2 ;;
    --project) project=${2:-}; shift 2 ;;
    --model) model=${2:-}; shift 2 ;;
    --effort) effort=${2:-}; shift 2 ;;
    --worktree) worktree=1; shift ;;
    --pi) kind=pi; shift ;;
    *) usage ;;
  esac
done
[ -n "$name" ] && [ -n "$note" ] || usage

if [ -z "${ORB_PANE_ID:-}" ]; then
  echo "start.sh: not inside an orb pane" >&2
  exit 1
fi
case "$note" in
  /*) ;;
  *) echo "start.sh: the note path must be absolute: $note" >&2; exit 1 ;;
esac
if [ ! -f "$note" ]; then
  echo "start.sh: no note at $note" >&2
  exit 1
fi

field() {
  python3 -c 'import json, sys; print(json.loads(sys.argv[1]).get(sys.argv[2], ""))' "$1" "$2" 2>/dev/null
}

create=(orb session create --name "$name")
[ -n "$project" ] && create+=(--project "$project")
[ -n "$worktree" ] && create+=(--worktree)

reply=""
for attempt in 1 2 3 4 5 6; do
  if reply=$("${create[@]}" 2>&1); then break; fi
  if [ "$(field "$reply" code)" != "starting" ] || [ "$attempt" -eq 6 ]; then
    echo "start.sh: orb refused to create the session" >&2
    echo "$reply" >&2
    exit 1
  fi
  sleep 5
done
session=$(field "$reply" session)
pane=$(field "$reply" pane)
if [ -z "$session" ] || [ -z "$pane" ]; then
  echo "start.sh: could not read the new session from orb's reply: $reply" >&2
  exit 1
fi
echo "session=$session pane=$pane"

fail() {
  echo "start.sh: $1 (session $session, pane $pane)" >&2
  exit "${2:-1}"
}

orb pane rename "$pane" "$name" >/dev/null || fail "could not rename the pane"

if [ "$kind" = pi ]; then
  flags=()
  [ -n "$model" ] && flags+=(--model "$model")
  [ -n "$effort" ] && flags+=(--thinking "$effort")
  orb agent start --kind pi --pane "$pane" --name "$name" -- ${flags[@]+"${flags[@]}"} >/dev/null || fail "pi did not start"
else
  command="claude --dangerously-skip-permissions"
  [ -n "$model" ] && command="$command --model $model"
  [ -n "$effort" ] && command="$command --effort $effort"
  [ "$(uname)" = Darwin ] && command="caffeinate $command"
  orb pane run "$pane" "$command" >/dev/null || fail "could not type the claude command"
  seen=""
  for _ in $(seq 1 15); do
    if orb agent get "$pane" >/dev/null 2>&1; then seen=1; break; fi
    sleep 2
  done
  if [ -z "$seen" ]; then
    echo "start.sh: orb saw no agent in pane $pane after 30 s. The pane shows:" >&2
    orb pane read "$pane" --tail 25 >&2
    echo "start.sh: the note was not sent (session $session, pane $pane)" >&2
    exit 3
  fi
fi

orb agent prompt "$pane" "Read $note and follow it." >/dev/null || fail "the agent started but the note was not sent"
echo "sent=$note"
