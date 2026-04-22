# kronn-docs sidecar

Python sidecar that converts HTML (or structured JSON) into PDF / DOCX /
XLSX / CSV / PPTX. Spawned by the Kronn Rust backend at startup — the
frontend never talks to it directly, every request goes through the
`/api/docs/*` routes on the main backend.

## Setup

From the repo root:

```sh
make docs-setup
```

What it does:

1. Creates a venv at `~/.kronn/venv/docs` (outside the repo so it survives
   `git clean`).
2. Installs the pinned deps from `pyproject.toml` with `pip install -e .`.
3. Installs WeasyPrint's system deps (cairo, pango, gdk-pixbuf) if
   missing — prints apt/brew/winget hints otherwise.

Once set up, Kronn auto-spawns the sidecar on next backend restart.
Remove the venv to opt out: `rm -rf ~/.kronn/venv/docs`.

## Endpoints

Every request body carries `output_path` — the Rust backend decides
where files land. The sidecar never picks paths itself.

| Method | Path | Body                                      | Phase |
|--------|------|-------------------------------------------|-------|
| GET    | /health | —                                      | 0     |
| POST   | /pdf | `{html, output_path, base_url?, page_size?}` | 0     |
| POST   | /docx | (phase 1)                                | 1     |
| POST   | /xlsx | (phase 1)                                | 1     |
| POST   | /csv | (phase 1)                                | 1     |
| POST   | /pptx | (phase 1)                                | 1     |

## Ready signal

On boot the sidecar prints `KRONN_DOCS_READY <port>` to stdout once
uvicorn is listening. The Rust spawner waits for that marker (max 5s)
instead of polling `/health`.
