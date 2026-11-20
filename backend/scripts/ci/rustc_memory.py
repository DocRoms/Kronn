#!/usr/bin/env python3
"""Fail when compiling one backend crate needed more memory than a runner has to spare.

`scripts/ci/rustc_one_at_a_time.sh` appends one line per rustc process of this
repository's crates: `<peak resident kB> <crate name>[ test]`. This gate keeps
the highest peak of each crate and fails above MAX_GB: the hosted runner has
16 GB, and a compilation that swaps does not finish within the job's time limit.
The size gate (crate_size.py) stops a crate early; this one measures the cost.
The cap sits above the highest peak measured in CI (5.76 GB for a crate of
184 000 lines) by what a crate at the size limit and run-to-run variation need.

usage: rustc_memory.py <log> [--max-gb N]
"""
import pathlib
import re
import sys

MAX_GB = 7.0
LINE = re.compile(r'^(\d+) ([A-Za-z0-9_]+(?: test)?)$')


def peaks(text):
    """Highest peak in kB per `crate` / `crate test`; other lines are ignored."""
    out = {}
    for line in text.splitlines():
        m = LINE.match(line.strip())
        if m:
            out[m.group(2)] = max(out.get(m.group(2), 0), int(m.group(1)))
    return out


def over(found, max_gb):
    return sorted(name for name, kb in found.items() if kb > max_gb * 1024 * 1024)


def main(argv):
    args = list(argv)
    max_gb = MAX_GB
    if '--max-gb' in args:
        at = args.index('--max-gb')
        max_gb = float(args[at + 1])
        del args[at:at + 2]
    if not args:
        print(__doc__)
        return 2
    log = pathlib.Path(args[0])
    found = peaks(log.read_text(encoding='utf-8', errors='replace')) if log.exists() else {}
    if not found:
        print('no crate of this repository was compiled (cache hit): nothing measured')
        return 0
    for name, kb in sorted(found.items(), key=lambda item: -item[1]):
        print(f'{kb / 1024 / 1024:6.2f} GB  {name}')
    bad = over(found, max_gb)
    if bad:
        print(f'\nover {max_gb:g} GB: {", ".join(bad)}. Split the crate or move modules to '
              'another one (docs/testing-quality.md, "Crate size").')
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
