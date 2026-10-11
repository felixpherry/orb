#!/usr/bin/env python3
"""Doc rules for orb.

- Name check: every code name AGENTS.md and the Record put in backticks
  (outside fenced blocks) must exist in orb's code.
- Architecture deny-list: AGENTS.md section 3 must not describe a bus.
- Dropped results: production Rust code must not `let _ =` a kill or removal,
  nor drop an error report with a `|_report|` closure.
- zmx list sites: each file's production `zmx … .list(` count must match the
  reviewed ZMX_LIST_SITES, so a new site fails until someone checks that a
  failed list there keeps panes rather than closing or killing them.

`--self-test` plants a bad name, a bad phrase, a dropped kill, a dropped
report and an unreviewed zmx list site, and exits 0 only if all are reported.
"""

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOCS = ["AGENTS.md", ".agents/RECORD.md"]
CODE_DIRS = ["crates", "src"]

# Names the docs may mention that orb's code doesn't have.
ALLOWLIST = {
    "ActorHost": "style-guide example in AGENTS.md §2",
    "ActorHostError": "style-guide example in AGENTS.md §2",
    "MessageSink": "style-guide example in AGENTS.md §2",
    "RSTEST_TIMEOUT": "rstest's environment variable",
    "clippy::missing_assert_message": "clippy lint name",
    "git diff": "developer command in AGENTS.md §7, not one orb runs",
    "replace_all": "example inside the Record's own format template",
}

SECTION_START = "## 3. Architecture"
SECTION_END = "## 4. Tests"
DENY_PHRASES = ["actor host", "pub/sub", "message bus"]
DENY_WORD = re.compile(r"\bbus\b", re.IGNORECASE)

# Production code that may drop a kill or removal result.
LET_DROP_ALLOWLIST = {
    ("crates/orb-term/src/pane.rs", ".kill("): "dropping a pane's attach client kills it; a client already gone is fine",
    ("crates/orb-domain/src/common/child.rs", ".kill("): "a timed-out helper is killed; its error is the timeout already returned",
}
# The whole statement up to its `;`, so a `let _ =` whose call is on a later line still matches.
LET_DROP = re.compile(
    r"let _ =[^;]*?(\.kill\(|remove_worktree|delete_branch|remove_dir_all|remove_file|delete_)",
    re.S,
)
DROPPED_REPORT = re.compile(r"\|_report\|")
TEST_MODULE = re.compile(r"^#\[cfg\(test\)\]", re.M)

# Production `zmx list` call sites per file, each reviewed to mean "keep" on error.
ZMX_LIST = re.compile(r"zmx\s*\.list\(")
ZMX_LIST_SITES = {
    "crates/orb-tui/src/run.rs": 4,
    # classify_exits: error -> None -> the pane respawns, never ends
    #   (exited_client_with_unlistable_zmx_keeps_its_pane, classify_exit's zmx_unknown case)
    # plan_resumes: error -> None -> nothing typed (resume_plan_types_nothing_where_zmx_couldnt_list)
    # reattach_live sweep: error -> empty -> nothing killed (stale_sessions_kills_nothing_when_zmx_lists_nothing)
    # reattach_live running: error -> empty -> panes come back as fresh shells, no kill
    #   (start_attaches_a_session_whose_panes_all_died)
    "crates/orb-domain/src/feat/sessions/sessions_actor.rs": 2,
    # poll: error -> that dir's panes left as they were (poll_leaves_a_pane_thread_as_it_was_when_zmx_cant_list)
    # kill_settled: error -> empty -> nothing killed (kill_settled_kills_nothing_when_zmx_cant_list)
}

SPAN = re.compile(r"`([^`\n]+)`")
IDENT = re.compile(r"^[A-Za-z_]\w*(::\w+)*$")
SUBCOMMAND = re.compile(r"^(claude|zmx|git|pi) ([a-z][a-z-]+)")


def in_code(*rg_args):
    result = subprocess.run(
        ["rg", "-q", *rg_args, "--", *CODE_DIRS], cwd=ROOT, capture_output=True
    )
    if result.returncode > 1:
        sys.exit(f"lint-rules: rg failed: {result.stderr.decode().strip()}")
    return result.returncode == 0


def span_ok(span):
    if IDENT.match(span) and (re.search(r"[A-Z_]", span) or "::" in span):
        return in_code("-w", "-F", span.split("::")[-1])
    sub = SUBCOMMAND.match(span)
    if sub:
        return in_code("-F", f'"{sub.group(2)}"')
    return True


def check_names(label, text):
    """Report backticked code names outside fenced blocks that orb's code doesn't have."""
    reports = []
    fenced = False
    for number, line in enumerate(text.splitlines(), start=1):
        if line.startswith("```"):
            fenced = not fenced
            continue
        if fenced:
            continue
        for span in SPAN.findall(line):
            if span in ALLOWLIST:
                continue
            if not span_ok(span):
                reports.append(f"{label}:{number}: unknown name `{span}`")
    return reports


def check_section3(label, text):
    """Report bus wording in the AGENTS.md architecture section."""
    lines = text.splitlines()
    try:
        start = lines.index(SECTION_START)
    except ValueError:
        return [f"{label}: section header `{SECTION_START}` not found"]
    end = next(
        (i for i in range(start + 1, len(lines)) if lines[i] == SECTION_END),
        len(lines),
    )
    reports = []
    for number in range(start, end):
        lowered = lines[number].lower()
        found = [p for p in DENY_PHRASES if p in lowered]
        found += [m.group(0) for m in DENY_WORD.finditer(lines[number])]
        reports += [f"{label}:{number + 1}: §3 must not say `{word}`" for word in found]
    return reports


def production(text):
    """The text before a file's first `#[cfg(test)]` line."""
    test = TEST_MODULE.search(text)
    return text[: test.start()] if test else text


def check_drops(label, text):
    """Report dropped kill/removal results and dropped error reports in production code."""
    code = production(text)
    reports = []
    for found in LET_DROP.finditer(code):
        if (label, found.group(1)) in LET_DROP_ALLOWLIST:
            continue
        number = code.count("\n", 0, found.start()) + 1
        reports.append(f"{label}:{number}: dropped kill/remove result")
    for found in DROPPED_REPORT.finditer(code):
        number = code.count("\n", 0, found.start()) + 1
        reports.append(f"{label}:{number}: dropped error report")
    return reports


def check_zmx_lists(label, text):
    """Report a file whose production `zmx … .list(` count differs from the reviewed list."""
    found = len(ZMX_LIST.findall(production(text)))
    want = ZMX_LIST_SITES.get(label, 0)
    if found == want:
        return []
    return [
        f"{label}: {found} zmx list call sites, reviewed {want}; "
        "check that a failed list there keeps, then update ZMX_LIST_SITES"
    ]


def rust_files():
    for code_dir in CODE_DIRS:
        for path in sorted((ROOT / code_dir).rglob("*.rs")):
            if "target" not in path.relative_to(ROOT).parts:
                yield path


def self_test():
    names = check_names("self-test", "Uses `NoSuchTypeForSelfTest` here.\n")
    deny = check_section3(
        "self-test", f"{SECTION_START}\nEvents go on the message bus.\n{SECTION_END}\n"
    )
    kill = check_drops("self-test", "fn f() {\n    let _ = zmx\n        .kill(&s);\n}\n")
    report = check_drops("self-test", "fn f() {\n    x.map_err(|_report| X);\n}\n")
    zmx = check_zmx_lists("self-test", "fn f() {\n    self.zmx\n        .list(dir);\n}\n")
    for line in names + deny + kill + report + zmx:
        print(line)
    if not names or not deny or not kill or not report or not zmx:
        print("self-test: a check did not fail on its planted text")
        return 1
    print("self-test: every check reported its planted text")
    return 0


def main():
    if "--self-test" in sys.argv[1:]:
        return self_test()
    reports = []
    for doc in DOCS:
        reports += check_names(doc, (ROOT / doc).read_text())
    reports += check_section3("AGENTS.md", (ROOT / "AGENTS.md").read_text())
    for path in rust_files():
        rel = str(path.relative_to(ROOT))
        text = path.read_text()
        reports += check_drops(rel, text)
        reports += check_zmx_lists(rel, text)
    for report in reports:
        print(report)
    if reports:
        return 1
    print("lint-rules: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
