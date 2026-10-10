#!/usr/bin/env python3
"""Every ts-rs export writes `<Name>.ts`: two exports with one name overwrite
each other, in whatever order the tests ran."""

from __future__ import annotations

import collections
import pathlib
import re
import unittest

SRC = pathlib.Path(__file__).resolve().parents[1] / "src"
ITEM = re.compile(r"\s*pub(?:\([^)]*\))?\s+(?:struct|enum|type)\s+(\w+)")
RENAME = re.compile(r'#\[ts\(.*\brename\s*=\s*"([^"]+)"')


def exported_names() -> dict[str, list[str]]:
    names: dict[str, list[str]] = collections.defaultdict(list)
    for path in sorted(SRC.rglob("*.rs")):
        lines = path.read_text(encoding="utf-8").splitlines()
        for index, line in enumerate(lines):
            if "#[ts(export" not in line:
                continue
            renamed = RENAME.search(line)
            for offset, following in enumerate(lines[index + 1 : index + 15], start=index + 2):
                renamed = RENAME.search(following) or renamed
                item = ITEM.match(following)
                if item:
                    name = renamed.group(1) if renamed else item.group(1)
                    names[name].append(f"{path.relative_to(SRC)}:{offset}")
                    break
    return names


class TsExportNameTests(unittest.TestCase):
    def test_every_export_has_its_own_file_name(self):
        names = exported_names()
        self.assertGreater(len(names), 500)
        duplicates = {name: where for name, where in names.items() if len(where) > 1}
        self.assertEqual(duplicates, {})


if __name__ == "__main__":
    unittest.main()
