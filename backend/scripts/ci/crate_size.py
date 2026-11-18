#!/usr/bin/env python3
"""Fail when a backend crate grows past the size one compilation may hold.

One rustc process compiles one crate, and its peak memory follows the crate's
size: the instrumented test build of a 875k-line crate needed 17 to 21 GB and
was killed on the CI runner. Each crate of `backend/` (its library root and
every module file reached from it, tests included) must stay under MAX_LINES.

usage: crate_size.py [backend-dir] [--max N]
"""
import os
import pathlib
import re
import sys
import tomllib

MAX_LINES = 200_000
MOD = re.compile(r'^(\s*)(?:pub(?:\([a-z: ]+\))? )?mod (\w+)\s*(;|\{)')
PATH = re.compile(r'#\[path = "([^"]+)"\]')


def read(path):
    return pathlib.Path(path).read_text(encoding='utf-8', errors='replace')


def module_files(root):
    """Every source file of the crate whose root is `root`."""
    seen = set()

    def walk(path, mod_rs):
        path = os.path.normpath(path)
        if path in seen or not os.path.isfile(path):
            return
        seen.add(path)
        folder = os.path.dirname(path)
        base = folder if mod_rs else os.path.join(folder, os.path.basename(path)[:-3])
        lines = read(path).split('\n')
        inline = []                                   # (indent, name) of open inline modules
        for i, line in enumerate(lines):
            text = line.strip()
            indent = len(line) - len(line.lstrip())
            while inline and text and indent <= inline[-1][0]:
                inline.pop()
            found = MOD.match(line)
            if not found:
                continue
            name = found.group(2)
            if found.group(3) == '{':
                inline.append((indent, name))
                continue
            explicit = None
            j = i - 1
            while j >= 0 and lines[j].lstrip().startswith('#['):
                hit = PATH.search(lines[j])
                explicit = explicit or (hit.group(1) if hit else None)
                j -= 1
            here = os.path.join(base, *[n for _, n in inline]) if inline else base
            if explicit:
                candidates = [os.path.join(here if inline else folder, explicit)]
            else:
                candidates = [os.path.join(here, name + '.rs'), os.path.join(here, name, 'mod.rs')]
            for candidate in candidates:
                if os.path.isfile(candidate):
                    walk(candidate, os.path.basename(candidate) == 'mod.rs')
                    break

    walk(root, True)
    return seen


def crates(backend):
    """(package name, library root) of every workspace member."""
    manifest = tomllib.loads(read(os.path.join(backend, 'Cargo.toml')))
    folders = ['.']
    for member in manifest.get('workspace', {}).get('members', []):
        if member.endswith('/*'):
            parent = os.path.join(backend, member[:-2])
            folders += [os.path.join(member[:-2], d) for d in sorted(os.listdir(parent))
                        if os.path.isfile(os.path.join(parent, d, 'Cargo.toml'))]
        elif member != '.':
            folders.append(member)
    out = []
    for folder in folders:
        path = os.path.join(backend, folder)
        package = tomllib.loads(read(os.path.join(path, 'Cargo.toml')))
        root = package.get('lib', {}).get('path', 'src/lib.rs')
        out.append((package['package']['name'], os.path.normpath(os.path.join(path, root))))
    return out


def measure(backend):
    return [(name, sum(read(f).count('\n') + 1
                       for f in module_files(root)))
            for name, root in crates(backend)]


def main(argv):
    args = [a for a in argv if not a.startswith('--')]
    backend = args[0] if args else str(pathlib.Path(__file__).resolve().parents[2])
    limit = int(argv[argv.index('--max') + 1]) if '--max' in argv else MAX_LINES
    args = [a for a in args if a != str(limit)]
    sizes = measure(backend)
    over = [(n, s) for n, s in sizes if s > limit]
    for name, size in sizes:
        print(f'{name:12} {size:>8} lines{"  <-- over " + str(limit) if size > limit else ""}')
    if over:
        print(f'\n{len(over)} crate(s) over {limit} lines: move modules to another crate '
              '(docs/testing-quality.md, "Crate size").', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
