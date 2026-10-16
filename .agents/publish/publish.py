#!/usr/bin/env python3
"""Build the public orb repo from a private ref: author, messages, paths, home paths.

  publish.py [--ref main] [--out DIR] [--email ADDR] [--skip-tree-checks] [--push URL]
  publish.py --check-only DIR [--email ADDR] [--skip-tree-checks]
  publish.py --map-only [--ref REF] [--map FILE]

See README.md next to this file.
"""

import argparse
import collections
import importlib.machinery
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import types
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_MAP = HERE / "messages.jsonl"
BOUNDARY = "f8fdb020593519e4a54499a7cb9b7326d4db8bd0"  # first commit with orb's own AGENTS.md
PLACEHOLDER_EMAIL = "0+zqccer@users.noreply.github.com"
NOREPLY = re.compile(r"^[0-9]+\+[A-Za-z0-9-]+@users\.noreply\.github\.com$")
PRIVATE_IDENTITY = "Felix Pherry <123felixpherry@gmail.com>"
AGENTS_POINTER = b"The coding-agent guide is in the latest AGENTS.md.\n"
# Literals run first, in this order, then the regex (filter-repo's order).
REPLACE_TEXT = """\
/Users/felixpherry==>/Users/me
-Users-felixpherry-==>-Users-me-
/home/felixp==>/home/me
felixpherry-6c==>me-6c
felixpherry==>zqccer
regex:(?s)\\n*<!-- private -->.*?<!-- /private -->==>
"""
PUBLIC_DOCS = (b"docs/demo/", b"docs/dev/")
SHOWN = 20  # failure lines printed per check

CONVENTIONAL = re.compile(
    r"^(feat|fix|refactor|perf|test|docs|build|ci|chore|style)"
    r"(\([a-z0-9-]+\))?: [a-z0-9`'\"<(].*[^.\s]$"
)
MERGE = re.compile(r"^Merge branch 'main' into [a-z0-9-]+$")
PHASE = re.compile(r"(?i)\bphase \d")
SCOPE = re.compile(r"^[a-z]+(?:\(([a-z0-9-]+)\))?: ")


def git(*args, cwd=None, check=True):
    done = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True)
    if check and done.returncode != 0:
        sys.exit(f"git {' '.join(args[:3])}… failed:\n{done.stderr}")
    return done


def message_problems(message):
    """Why a commit message breaks the rules; empty when it passes."""
    lines = message.rstrip("\n").split("\n")
    subject = lines[0]
    problems = []
    if not (CONVENTIONAL.match(subject) or MERGE.match(subject)):
        problems.append("not a conventional subject or a merge row")
    if len(subject) > 72:
        problems.append(f"subject is {len(subject)} chars (max 72)")
    if PHASE.search(subject):
        problems.append("names a phase")
    if len(lines) > 1 and lines[1] != "":
        problems.append("line 2 is not blank")
    return problems


def load_map(path):
    """Map rows as {bytes id: bytes message}, and a line per bad row."""
    mapping, bad = {}, []
    if not path.exists():
        print(f"note: no map at {path}; treating it as empty")
        return mapping, bad
    for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip():
            continue
        try:
            row = json.loads(line)
            cid, message = row["id"], row["message"]
        except (ValueError, KeyError, TypeError) as err:
            bad.append(f"{path.name}:{n}: not a JSON row with id and message ({err})")
            continue
        if not re.fullmatch(r"[0-9a-f]{40}", cid):
            bad.append(f"{path.name}:{n}: id {cid!r} is not a 40-char commit id")
            continue
        if cid.encode() in mapping:
            bad.append(f"{path.name}:{n}: duplicate id {cid}")
            continue
        for problem in message_problems(message):
            bad.append(f"{cid[:7]} {message.splitlines()[0] if message else ''!r}: {problem}")
        mapping[cid.encode()] = message.rstrip("\n").encode()
    return mapping, bad


def log_messages(ref, cwd=None):
    """(id, message) for every commit reachable from ref, newest first."""
    out = git("log", "--format=%H%x1f%B%x1e", ref, cwd=cwd).stdout
    pairs = []
    for record in out.split("\x1e"):
        record = record.lstrip("\n")
        if record:
            cid, message = record.split("\x1f", 1)
            pairs.append((cid, message))
    return pairs


def precheck(source, ref, mapping, allow_unmapped=False):
    """Every commit on ref has a row or already passes; returns failure lines."""
    failures = []
    missing = [c for c in mapping if git("cat-file", "-e", c.decode() + "^{commit}", cwd=source, check=False).returncode]
    failures += [f"{c.decode()[:7]}: map row names no commit in this repo" for c in missing]
    if ref is None or allow_unmapped:
        return failures
    for cid, message in log_messages(ref, cwd=source):
        if cid.encode() not in mapping and message_problems(message):
            failures.append(f"{cid[:7]} {message.splitlines()[0]!r}: needs a map row ({'; '.join(message_problems(message))})")
    return failures


def print_tally(source, ref, mapping):
    """Scope tally and every published subject grouped by scope, for the read-through."""
    if ref is None:
        rows = [(c.decode(), m.decode()) for c, m in mapping.items()]
    else:
        rows = [(c, mapping.get(c.encode(), m.encode()).decode()) for c, m in reversed(log_messages(ref, cwd=source))]
    def scope(message):
        if MERGE.match(message):
            return "(merge)"
        found = SCOPE.match(message)
        return (found.group(1) or "(none)") if found else "(bad)"
    rows.sort(key=lambda row: scope(row[1]))
    tally = collections.Counter(scope(m) for _, m in rows)
    print("scope tally:")
    for name, count in sorted(tally.items()):
        print(f"  {count:4} {name}")
    print("\nsubjects by scope:")
    for cid, message in rows:
        lines = message.rstrip("\n").split("\n")
        body = f"  [+{len(lines) - 2} body lines]" if len(lines) > 2 else ""
        print(f"  {cid[:7]} {lines[0]}{body}")
    print()


def keep_path(name):
    """filename_callback: drop private paths, keep the rest."""
    if name.startswith(b".agents/") or name.startswith(b".claude/"):
        return None
    if name.startswith(b"docs/") and not (name.startswith(PUBLIC_DOCS) or name == b"docs/keys.md"):
        return None
    return name


def load_filter_repo():
    path = shutil.which("git-filter-repo")
    if not path:
        sys.exit("git-filter-repo not found on PATH (brew install git-filter-repo)")
    loader = importlib.machinery.SourceFileLoader("git_filter_repo", path)
    module = types.ModuleType(loader.name)
    loader.exec_module(module)
    return module


def build(source, ref, out, email, mapping):
    git("clone", "--quiet", "--no-local", "--single-branch", "--branch", ref, str(source), str(out))
    raw = git("log", "--full-history", "-m", "--raw", "--no-abbrev", "--format=", BOUNDARY + "^", "--", "AGENTS.md", cwd=out).stdout
    old_agents = {line.split()[3].encode() for line in raw.splitlines() if line.startswith(":")} - {b"0" * 40}
    if not old_agents:
        sys.exit("found no AGENTS.md versions before the boundary commit")
    print(f"old AGENTS.md versions replaced: {len(old_agents)}")

    work = Path(tempfile.mkdtemp(prefix="orb-publish-"))
    (work / "mailmap").write_text(f"zqccer <{email}> {PRIVATE_IDENTITY}\n")
    (work / "replace-text").write_text(REPLACE_TEXT)

    def blob_callback(blob, _metadata):
        if blob.original_id in old_agents:
            blob.data = AGENTS_POINTER

    def commit_callback(commit, _metadata):
        if commit.original_id in mapping:
            commit.message = mapping[commit.original_id] + b"\n"

    fr = load_filter_repo()
    args = fr.FilteringOptions.parse_args([
        "--mailmap", str(work / "mailmap"),
        "--replace-text", str(work / "replace-text"),
        "--replace-refs", "delete-no-add",
        "--prune-empty", "auto",
        "--prune-degenerate", "auto",
        "--quiet",
    ])
    cwd = os.getcwd()
    os.chdir(out)
    try:
        fr.RepoFilter(args, filename_callback=keep_path, blob_callback=blob_callback, commit_callback=commit_callback).run()
    finally:
        os.chdir(cwd)
    shutil.rmtree(work)

    if ref != "main":
        git("branch", "-M", ref, "main", cwd=out)
    git("checkout", "--quiet", "main", cwd=out)
    print("filter-repo", git("filter-repo", "--version").stdout.strip())
    print("tip", git("rev-parse", "main", cwd=out).stdout.strip(), "commits", git("rev-list", "--count", "main", cwd=out).stdout.strip())


def checks(out, email, tree):
    """Pre-push checks on a built repo; returns failure lines (empty = all pass)."""
    failures = []

    def fail(check, lines):
        lines = list(lines)
        if lines:
            failures.append(f"[{check}] {len(lines)} problem(s)")
            failures.extend("  " + line for line in lines[:SHOWN])
            if len(lines) > SHOWN:
                failures.append(f"  … {len(lines) - SHOWN} more")

    revs = git("rev-list", "--all", cwd=out).stdout.split()

    identities = set(git("log", "--all", "--format=%an <%ae>%n%cn <%ce>", cwd=out).stdout.splitlines())
    fail("1 identity", sorted(identities - {f"zqccer <{email}>"}))

    name_hits = git("grep", "-a", "-i", "-e", "felix", "-e", "pherry", *revs, cwd=out, check=False).stdout.splitlines()
    name_hits += [f"message: {line}" for line in git("log", "--all", "--format=%B", cwd=out).stdout.splitlines() if re.search(r"(?i)felix|pherry", line)]
    bad_paths = set()
    for rev in revs:
        for path in git("ls-tree", "-r", "--name-only", rev, cwd=out).stdout.splitlines():
            if re.search(r"(?i)felix|pherry", path) or keep_path(path.encode()) is None:
                bad_paths.add(path)
    fail("2 name", name_hits + [f"path: {p}" for p in sorted(bad_paths) if re.search(r"(?i)felix|pherry", p)])

    jinn = git("grep", "-l", "-i", "jinn", *revs, cwd=out, check=False).stdout.splitlines()
    jinn_paths = {line.split(":", 1)[1] for line in jinn}
    fail("3 jinn", sorted(p for p in jinn_paths if p != "README.md" and not p.startswith("crates/")))

    fail("4 paths", sorted(p for p in bad_paths if keep_path(p.encode()) is None))

    fail("5 private block", git("grep", "-F", "<!-- private", *revs, cwd=out, check=False).stdout.splitlines())

    fail("6 messages", [
        f"{cid[:7]} {message.splitlines()[0]!r}: {'; '.join(message_problems(message))}"
        for cid, message in log_messages("--all", cwd=out) if message_problems(message)
    ])

    if tree:
        for command in (["just", "lint"], ["just", "test"], ["sh", "scripts/test-install.sh"]):
            print("running", " ".join(command), flush=True)
            done = subprocess.run(command, cwd=out, capture_output=True, text=True)
            if done.returncode != 0:
                fail("7 tree", [f"{' '.join(command)} exited {done.returncode}"] + (done.stdout + done.stderr).splitlines()[-15:])
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--ref", help="branch to publish (default main; --map-only: none, rows only)")
    parser.add_argument("--out", type=Path, help="output dir; must not exist or be empty (default: a new temp dir)")
    parser.add_argument("--email", default=PLACEHOLDER_EMAIL, help="public GitHub noreply email")
    parser.add_argument("--map", type=Path, default=DEFAULT_MAP, help="message map (JSON Lines)")
    parser.add_argument("--skip-tree-checks", action="store_true", help="skip just lint, just test, test-install.sh")
    parser.add_argument("--push", metavar="URL", help="push main to URL after every check passes (never forced)")
    parser.add_argument("--map-only", action="store_true", help="check the map (and, with --ref, the ref's coverage)")
    parser.add_argument("--check-only", type=Path, metavar="DIR", help="run the checks on an already built repo")
    parser.add_argument("--allow-unmapped", action="store_true", help="smoke tests only: build even when commits lack a row (check 6 still fails)")
    opts = parser.parse_args()

    if not NOREPLY.match(opts.email):
        sys.exit(f"--email must be a GitHub noreply address (<id>+<user>@users.noreply.github.com), got {opts.email}")
    if opts.push and opts.email == PLACEHOLDER_EMAIL:
        sys.exit("--push refuses the placeholder email; pass --email with the account's real noreply address")
    if opts.push and opts.allow_unmapped:
        sys.exit("--push refuses --allow-unmapped")

    if opts.check_only:
        failures = checks(opts.check_only, opts.email, not opts.skip_tree_checks)
        print("\n".join(failures) or "every check passed")
        return 1 if failures else 0

    source = Path(git("rev-parse", "--show-toplevel").stdout.strip())
    mapping, bad_rows = load_map(opts.map)

    if opts.map_only:
        print_tally(source, opts.ref, mapping)
        failures = bad_rows + precheck(source, opts.ref, mapping)
        print("\n".join(failures) or f"map ok: {len(mapping)} rows")
        return 1 if failures else 0

    ref = opts.ref or "main"
    failures = bad_rows + precheck(source, ref, mapping, opts.allow_unmapped)
    if failures:
        print("map pre-check failed:\n" + "\n".join(failures))
        return 1

    out = opts.out or Path(tempfile.mkdtemp(prefix="orb-public-"))
    if out.exists() and any(out.iterdir()):
        sys.exit(f"--out {out} is not empty")
    if out.exists():
        out.rmdir()
    build(source, ref, out, opts.email, mapping)

    failures = checks(out, opts.email, not opts.skip_tree_checks)
    print("\n".join(failures) or "every check passed")
    print("output:", out)
    if failures:
        return 1
    if opts.push:
        done = subprocess.run(["git", "push", opts.push, "main"], cwd=out)
        return done.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
