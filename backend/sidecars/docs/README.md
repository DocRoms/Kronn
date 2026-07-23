# kronn-docs sidecar

Python sidecar that converts HTML (or structured JSON) into PDF / DOCX /
XLSX / CSV / PPTX. Spawned by the Kronn Rust backend at startup — the
frontend never talks to it directly, every request goes through the
`/api/docs/*` routes on the main backend.

## Distribution

Docker images bake the sidecar virtualenv into the image. Desktop builds
run `build_bundle.py` through `desktop/scripts/build-docs-sidecar.mjs`,
then Tauri includes the resulting standalone executable under
`sidecars/docs/`. Installed users do not run a setup command.

The release bundle is smoke-tested against both `/pdf` and `/docx`. On
macOS, the test masks Homebrew's dynamic-library path so it also proves the
exporter uses the bundled Pango/Fontconfig stack instead of dependencies from
the CI build machine. The custom Pillow hook keeps Pillow's private
HarfBuzz/FreeType/libpng copies from replacing Pango's ABI-compatible copies.
The DOCX smoke test also verifies that the frozen exporter embeds a rendered
page and configures edge-to-edge Word pages.

PDF exports use no implicit page margin unless the HTML declares one in
`@page`; spacing authored inside the HTML remains intact. DOCX exports render
the same HTML/CSS through WeasyPrint and place every resulting page edge to
edge in Word. This preserves CSS variables, gradients, grid/flex layouts,
positioned elements, and other browser styling. The Word result is visually
fixed rather than editable text because Word cannot represent arbitrary
browser layouts faithfully.

## Source-development setup

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

When a frozen desktop exporter already exists under
`desktop/src-tauri/resources/docs-sidecar/`, the native backend uses it as a
fallback. This keeps `kronn start-dev` aligned with the packaged application
without requiring a second sidecar installation.

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
uvicorn is listening. The Rust spawner waits for that marker (max 15s)
instead of polling `/health`.
