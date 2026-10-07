#!/usr/bin/env python3
"""Keep the tech-debt index and its detail files in step.

docs/inconsistencies-tech-debt.md lists open items only, one table row per
ID; docs/tech-debt/<ID>.md holds the details. This check fails when a row has
no detail file, a detail file has no row, a row is marked closed, or a detail
file's Status says the item is fixed.
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
INDEX = ROOT / "docs" / "inconsistencies-tech-debt.md"
DETAILS = ROOT / "docs" / "tech-debt"

# Rows allowed without a detail file while another change lands one.
PENDING_REMOVAL: set[str] = set()

_ROW_RE = re.compile(r"^\|\s*(TD-\d{8}-[a-z0-9-]+)\s*\|(.*)\|\s*$")
_STATUS_RE = re.compile(r"\*\*Status\*\*\s*:\s*(.*)", re.I)
_CLOSED_WORD = re.compile(r"^(fixed|resolved|closed|done)\b", re.I)


def index_rows(text: str) -> dict[str, list[str]]:
    rows: dict[str, list[str]] = {}
    for line in text.splitlines():
        match = _ROW_RE.match(line.strip())
        if match:
            rows[match.group(1)] = [cell.strip() for cell in match.group(2).split("|")]
    return rows


def _says_closed(text: str) -> bool:
    # Skip emphasis and status emoji before the first word.
    return bool(_CLOSED_WORD.match(re.sub(r"^[^A-Za-z]+", "", text)))


def problems(index_text: str, details: dict[str, str]) -> list[str]:
    rows = index_rows(index_text)
    found = []
    for td_id, cells in sorted(rows.items()):
        if td_id not in details and td_id not in PENDING_REMOVAL:
            found.append(f"{td_id}: row without docs/tech-debt/{td_id}.md")
        severity = cells[-1] if cells else ""
        problem = cells[0] if cells else ""
        if _says_closed(severity) or _says_closed(problem):
            found.append(f"{td_id}: closed row; remove it and its detail file")
    for td_id, body in sorted(details.items()):
        if td_id not in rows:
            found.append(f"docs/tech-debt/{td_id}.md: no row in the index")
        status = _STATUS_RE.search(body)
        if status and _says_closed(status.group(1)):
            found.append(f"docs/tech-debt/{td_id}.md: Status says closed; remove the file and its row")
    return found


def main() -> int:
    details = {path.stem: path.read_text(encoding="utf-8") for path in DETAILS.glob("TD-*.md")}
    found = problems(INDEX.read_text(encoding="utf-8"), details)
    for line in found:
        print(line)
    if found:
        return 1
    print(f"tech-debt registry consistent: {len(details)} detail files")
    return 0


if __name__ == "__main__":
    sys.exit(main())
