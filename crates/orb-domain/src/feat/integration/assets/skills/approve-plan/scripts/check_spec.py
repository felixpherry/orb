"""Check a plan folder's plan.md and tasks.md against the rules a script can see.

Usage: python3 -I check_spec.py <plan folder>
Prints one line per problem as <file>:<line>: <message> and exits 1, or prints nothing and exits 0.
"""
import re
import sys
from pathlib import Path

REQUIRED = [
    "Problem",
    "Solution",
    "Acceptance Criteria",
    "Phases",
    "Dialectical Outcomes",
    "Relevant Files",
    "Key Code Context",
    "Implementation Algorithm",
    "Anti-Goals",
    "Edge Cases & Gotchas",
    "Navigation Anchors",
    "Dependency Mappings",
    "Test Strategies",
]
HEADING = re.compile(r"^(#{1,6})\s+(.*\S)\s*$")
TASK = re.compile(r"^\s*[-*]\s+\[[ xX]\]\s+(.*)$")
FENCE = re.compile(r"^\s*(```|~~~)")


def outside_fences(text):
    fenced = False
    for number, line in enumerate(text.splitlines(), 1):
        if FENCE.match(line):
            fenced = not fenced
            continue
        if not fenced:
            yield number, line


def headings(text, level):
    found = []
    for number, line in outside_fences(text):
        match = HEADING.match(line)
        if match and len(match.group(1)) == level:
            found.append((number, match.group(2)))
    return found


def names(title, wanted):
    return title.lower().startswith(wanted.lower())


def check_plan(text):
    problems = []
    sections = headings(text, 2)
    for wanted in REQUIRED:
        if not any(names(title, wanted) for _, title in sections):
            problems.append(("plan.md", 1, f'no "## {wanted}" section'))
    for position, wanted in enumerate(["Problem", "Solution"]):
        if len(sections) > position and not names(sections[position][1], wanted):
            line, title = sections[position]
            problems.append(("plan.md", line, f'section {position + 1} is "{title}": the spec opens with Problem, then Solution'))
    for number, line in outside_fences(text):
        if TASK.match(line):
            problems.append(("plan.md", number, "checkbox in the spec: tasks.md tracks progress"))
    return problems, any(names(title, "Record Updates") for _, title in sections)


def check_tasks(text, plan_has_record):
    problems = []
    phases = []
    for number, line in outside_fences(text):
        heading = HEADING.match(line)
        task = TASK.match(line)
        if heading and len(heading.group(1)) == 2:
            phases.append({"line": number, "title": heading.group(2), "tasks": []})
        elif task and phases:
            phases[-1]["tasks"].append((number, task.group(1)))
        elif task:
            problems.append(("tasks.md", number, "task above the first ## phase header"))
    if not phases:
        return problems + [("tasks.md", 1, "no ## phase headers")]
    for phase in phases:
        if not phase["tasks"]:
            problems.append(("tasks.md", phase["line"], f'phase "{phase["title"]}" has no tasks'))
    last = phases[-1]
    if last["title"].strip().lower() != "verification":
        problems.append(("tasks.md", last["line"], f'the last phase is "{last["title"]}": it is Verification'))
        return problems
    if len(phases) == 1:
        problems.append(("tasks.md", last["line"], "Verification is the only phase: the plan's phases come first"))
    record_tasks = [(number, text) for number, text in last["tasks"] if "record updates" in text.lower()]
    if plan_has_record and not record_tasks:
        problems.append(("tasks.md", last["line"], "the spec has Record Updates but Verification has no Record Updates task"))
    if not plan_has_record and record_tasks:
        problems.append(("tasks.md", record_tasks[0][0], "Record Updates task, but the spec has no Record Updates section"))
    if record_tasks and last["tasks"] and record_tasks[-1] != last["tasks"][-1]:
        problems.append(("tasks.md", record_tasks[-1][0], "the Record Updates task is not the last task"))
    return problems


def main():
    if len(sys.argv) != 2:
        print(__doc__.strip(), file=sys.stderr)
        sys.exit(2)
    folder = Path(sys.argv[1])
    problems = []
    plan_has_record = False
    plan = folder / "plan.md"
    tasks = folder / "tasks.md"
    if plan.is_file():
        found, plan_has_record = check_plan(plan.read_text())
        problems += found
    else:
        problems.append(("plan.md", 1, "file is missing"))
    if tasks.is_file():
        problems += check_tasks(tasks.read_text(), plan_has_record)
    else:
        problems.append(("tasks.md", 1, "file is missing"))
    for name, line, message in sorted(set(problems)):
        print(f"{name}:{line}: {message}")
    sys.exit(1 if problems else 0)


if __name__ == "__main__":
    main()
