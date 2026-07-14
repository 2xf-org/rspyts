"""
Docs link checker: every relative link in tracked markdown must resolve.

Complements the generated-code drift gate: rustdoc (-D warnings) keeps API
docs honest against the code, and this keeps the markdown graph honest
against the tree. Anchors are checked for same-file `#fragment` links and
for `file.md#fragment` links against the target's headings.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")
HEADING = re.compile(r"^#{1,6}\s+(.*)$", re.MULTILINE)
SKIP_PREFIXES = ("http://", "https://", "mailto:")


def anchors(md: str) -> set[str]:
    """
    GitHub-style anchor slugs for every heading in a markdown document.
    """
    slugs = set()
    for raw in HEADING.findall(md):
        text = re.sub(r"[`*_]", "", raw).strip().lower()
        text = re.sub(r"[^\w\s§.-]", "", text, flags=re.UNICODE)
        slugs.add(re.sub(r"\s+", "-", text).replace(".", ""))
    return slugs


def check(path: Path) -> list[str]:
    body = path.read_text(encoding="utf-8")
    problems = []
    for target in LINK.findall(body):
        if target.startswith(SKIP_PREFIXES):
            continue
        raw, _, fragment = target.partition("#")
        dest = (path.parent / raw).resolve() if raw else path
        if raw and not dest.exists():
            problems.append(f"{path}: broken link -> {target}")
            continue
        if fragment and dest.suffix == ".md" and dest.exists():
            if fragment.lower() not in anchors(dest.read_text(encoding="utf-8")):
                problems.append(f"{path}: missing anchor -> {target}")
    return problems


def main() -> int:
    tracked = subprocess.run(
        ["git", "ls-files", "*.md"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.split()
    problems = [p for name in tracked for p in check(Path(name))]
    for problem in problems:
        print(problem)
    print(f"checked {len(tracked)} markdown files: {len(problems)} problem(s)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
