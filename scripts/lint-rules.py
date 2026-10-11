#!/usr/bin/env python3
"""Doc rules for orb.

- Name check: every code name AGENTS.md and the Record put in backticks
  (outside fenced blocks) must exist in orb's code.
- Architecture deny-list: AGENTS.md section 3 must not describe a bus.

`--self-test` plants a bad name and a bad phrase and exits 0 only if both are reported.
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


def self_test():
    names = check_names("self-test", "Uses `NoSuchTypeForSelfTest` here.\n")
    deny = check_section3(
        "self-test", f"{SECTION_START}\nEvents go on the message bus.\n{SECTION_END}\n"
    )
    for report in names + deny:
        print(report)
    if not names or not deny:
        print("self-test: check did not fail on the planted name")
        return 1
    print("self-test: both checks reported the planted name and phrase")
    return 0


def main():
    if "--self-test" in sys.argv[1:]:
        return self_test()
    reports = []
    for doc in DOCS:
        reports += check_names(doc, (ROOT / doc).read_text())
    reports += check_section3("AGENTS.md", (ROOT / "AGENTS.md").read_text())
    for report in reports:
        print(report)
    if reports:
        return 1
    print("lint-rules: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
