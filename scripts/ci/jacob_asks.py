"""Shared scanning for the Jacob-ask registry checks.

One ask is one `[feedback <slug>]` tag in the app sources. Tags wrap across lines inside doc
comments, so every scan here runs on text with comment markers and newlines flattened — a
line-wrapped tag is the same tag.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path

IOS = Path("apps/ios/Arbos")
APP_DIR = IOS / "Arbos"
TEST_DIRS = (IOS / "ArbosTests", IOS / "ArbosUITests")
REGISTRY = Path("docs/product/jacob-asks.md")

TAG_RE = re.compile(r"\[feedback\s+([a-z0-9][a-z0-9-]*)\]")
FUNC_RE = re.compile(r"\bfunc\s+(test[A-Za-z0-9_]*)\s*\(")
SKIP_RE = re.compile(r"^(\s*)(?:try\s+)?throw\s+XCTSkip\s*\(", re.MULTILINE)
SUITE_RE = re.compile(r"\bclass\s+(\w+)\s*:\s*[\w.]*XCTestCase\b")


def flatten(text: str) -> str:
    """Comment markers and newlines gone, runs of space collapsed."""
    return re.sub(r"\s+", " ", re.sub(r"^\s*(///?|\*)", " ", text, flags=re.MULTILINE))


def swift_files(*roots: Path) -> list[Path]:
    return sorted(p for root in roots for p in root.rglob("*.swift"))


def app_tags(root: Path) -> dict[str, list[Path]]:
    """Every ask recorded in the app, and the files recording it."""
    found: dict[str, list[Path]] = {}
    for path in swift_files(root / APP_DIR):
        for tag in set(TAG_RE.findall(flatten(path.read_text(encoding="utf-8")))):
            found.setdefault(tag, []).append(path.relative_to(root))
    return found


@dataclass(eq=False)
class TestCase:
    suite: str
    name: str
    path: Path
    tags: set[str] = field(default_factory=set)
    skipped: bool = False
    """Skips however it is called. `XCTSkipIf("sources not beside the bundle")` and friends are
    conditional escapes that still run in CI, so only a `throw XCTSkip` at the body's own
    indentation counts — that one can never do anything but skip."""

    @property
    def qualified(self) -> str:
        return f"{self.suite}.{self.name}"


def _doc_start(text: str, start: int) -> int:
    """Where a declaration's own doc comment begins: the blank line above it."""
    blank = text.rfind("\n\n", 0, start)
    return blank + 1 if blank >= 0 else start


def _always_skips(text: str, start: int, end: int) -> bool:
    """A `throw XCTSkip` at the body's own indentation runs every time: the case is off."""
    body_indent = len(re.match(r"\s*", text[text.rfind("\n", 0, start) + 1 :]).group(0)) + 4
    return any(len(m.group(1)) <= body_indent for m in SKIP_RE.finditer(text, start, end))


def test_cases(root: Path) -> list[TestCase]:
    """Every test case, with the asks it names and whether it skips itself.

    A case's tags are those on the case plus those on its suite: a suite headed by an ask
    guards that ask in all of its cases. Several suites may share one file, so a suite's doc
    comment runs from the end of the one before it rather than from the top of the file.
    """
    out: list[TestCase] = []
    for path in swift_files(*(root / d for d in TEST_DIRS)):
        text = path.read_text(encoding="utf-8")
        rel = path.relative_to(root)
        suites = [(m.start(), m.group(1)) for m in SUITE_RE.finditer(text)]
        for i, (suite_at, suite) in enumerate(suites):
            suite_end = suites[i + 1][0] if i + 1 < len(suites) else len(text)
            doc_from = _doc_start(text, suite_at) if i else 0
            suite_tags = set(TAG_RE.findall(flatten(text[doc_from:suite_at])))
            starts = [(m.start(), m.group(1)) for m in FUNC_RE.finditer(text, suite_at, suite_end)]
            for k, (start, name) in enumerate(starts):
                # Up to the next case's doc comment, so a neighbour's tag is not borrowed.
                end = _doc_start(text, starts[k + 1][0]) if k + 1 < len(starts) else suite_end
                out.append(
                    TestCase(
                        suite=suite,
                        name=name,
                        path=rel,
                        tags=suite_tags | set(TAG_RE.findall(flatten(text[_doc_start(text, start) : end]))),
                        skipped=_always_skips(text, start, end),
                    )
                )
    return out


ROW_RE = re.compile(r"^\|\s*`([a-z0-9][a-z0-9-]*)`\s*\|(.+)\|\s*`([A-Za-z0-9_.]+)`\s*\|\s*$")
SUPERSEDED_HEADING = "## Superseded"
PENDING_HEADING = "## Waiting on a tag"


def read_registry(root: Path) -> tuple[dict[str, str], set[str], list[str]]:
    """Registered asks and their guards, the superseded ones, and any parse complaints.

    Rows under 'Waiting on a tag' and 'Superseded' are prose, not promises: an ask there has no
    tag to tie a guard to yet, or has been answered by a later ask.
    """
    path = root / REGISTRY
    if not path.exists():
        # A tree from before the register: it promised nothing, so nothing can be lost.
        return {}, set(), []
    text = path.read_text(encoding="utf-8")
    pending_at = text.find(PENDING_HEADING)
    superseded_at = text.find(SUPERSEDED_HEADING)
    if superseded_at < 0:
        return {}, set(), [f"{REGISTRY} has no '{SUPERSEDED_HEADING}' section"]

    live_end = pending_at if 0 <= pending_at < superseded_at else superseded_at
    guarded: dict[str, str] = {}
    problems: list[str] = []
    for line in text[:live_end].splitlines():
        if (m := ROW_RE.match(line)) is None:
            continue
        ask, guard = m.group(1), m.group(3)
        if ask in guarded:
            problems.append(f"{ask}: listed twice in {REGISTRY}")
        guarded[ask] = guard

    superseded = {
        m.group(1)
        for line in text[superseded_at:].splitlines()
        if (m := re.match(r"^\|\s*`([a-z0-9][a-z0-9-]*)`\s*\|", line))
    }
    return guarded, superseded, problems


def find_guard(name: str, cases: list[TestCase]) -> list[TestCase]:
    """The cases a guard names: a whole suite, or one `Suite.testCase`."""
    if "." in name:
        return [c for c in cases if c.qualified == name]
    return [c for c in cases if c.suite == name]
