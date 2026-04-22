---
name: Kronn Docs
description: Generate PDF / DOCX / XLSX / CSV / PPTX files directly from the conversation. Kronn ships a Python sidecar with WeasyPrint, python-docx, XlsxWriter and python-pptx — no external install or dependency juggling needed.
icon: 📄
category: domain
auto_triggers:
  common:
    # File-format tokens don't translate — one regex covers every language.
    - "\\b(pdf|docx?|xlsx?|pptx?|csv)\\b"
  fr:
    # Stems that cover the full conjugation space, incl. grave-accent
    # forms: "génère", "génères" → `génèr`; "générer", "généré",
    # "génération" → `génér`. Same trick for "crée/créer", "exporte/-r".
    - "\\b(gén[eéè]r\\w*|crée[rz]?|créer|exporte[rz]?|exporter|produi[rst]\\w*|rédig\\w*|écri[rts]\\w*).{0,40}(fichier|document|rapport|tableau|présentation|feuille)"
    - "\\b(word|excel|powerpoint|tableur)\\b"
  en:
    - "(generate|create|export|produce|write).{0,40}(file|document|report|spreadsheet|presentation|sheet)"
    - "\\b(word|excel|powerpoint|spreadsheet)\\b"
  es:
    - "(gener|crear?|exportar?|produ[zc]ir).{0,40}(archivo|documento|informe|hoja|presentación)"
    - "\\b(word|excel|powerpoint)\\b"
---

# Document generation — Kronn Docs

You have access to Kronn's built-in document generation endpoints. The
user doesn't need to install anything — Kronn ships a Python sidecar
(WeasyPrint + python-docx + XlsxWriter + python-pptx) that handles every
format out of the box. If the endpoints respond with
"Document sidecar unavailable" tell the user to run `make docs-setup`
once and restart Kronn; it's a one-time thing.

## Workflow — HTML preview + export (recommended)

For **PDF** and **DOCX**, compose the content as a complete HTML
document (with `<style>` if you need layout) and wrap it in a
`kronn-doc-preview` fenced code block. Kronn's chat UI detects the
fence, renders the HTML in a sandboxed preview, and shows export
buttons below — the user clicks to generate the final file.

````markdown
Here's the Jira annual report I put together. Review the preview below
and click **📄 PDF** to export when it looks right.

```kronn-doc-preview
<!DOCTYPE html>
<html>
<head>
<style>
  body { font-family: -apple-system, sans-serif; color: #1a1d23; }
  h1 { color: #0f766e; border-bottom: 2px solid #0f766e; }
  table { border-collapse: collapse; width: 100%; }
  th, td { padding: 8px; border: 1px solid #ddd; text-align: left; }
  th { background: #eef2f5; }
</style>
</head>
<body>
  <h1>Jira — Annual report 2025</h1>
  <p>Summary of 2,340 tickets across 14 projects...</p>
  <h2>Top 5 epics</h2>
  <table>
    <tr><th>Epic</th><th>Tickets</th><th>Status</th></tr>
    <tr><td>EW-1234 Dashboard rewrite</td><td>87</td><td>Done</td></tr>
    <!-- ... -->
  </table>
</body>
</html>
```
````

The user gets a live preview + `[📄 PDF]` and `[📝 DOCX]` buttons. No
need to call the endpoint yourself — the UI does it on click.

## Workflow — structured data export (XLSX / CSV / PPTX)

Spreadsheet and presentation formats take **JSON** input (rows × cols, or
slides), not HTML — an iframe preview would look awful, and the
spreadsheet/slide app is the rendering target anyway. Wrap the payload in
a `kronn-doc-data` fence with a `format` discriminator. Kronn's UI shows
a compact card with a summary (row count, sheet count, slide count) and
a single export button.

### CSV — flat tabular dump

````markdown
```kronn-doc-data
{
  "format": "csv",
  "rows": [
    ["Epic", "Tickets", "Status"],
    ["EW-1234 Dashboard rewrite", 87, "Done"],
    ["EW-2210 Search v2", 42, "In progress"]
  ]
}
```
````

Optional `delimiter` field (default `,`). First row is the header by
convention — nothing enforces it, but users expect it.

### XLSX — one or more sheets

````markdown
```kronn-doc-data
{
  "format": "xlsx",
  "sheets": [
    {
      "name": "Q1 2026",
      "rows": [
        ["Epic", "Tickets", "Status"],
        ["EW-1234", 87, "Done"]
      ]
    },
    { "name": "Q2 2026", "rows": [["..."]] }
  ]
}
```
````

Sheet names are capped at 31 chars and stripped of `\ / ? * [ ] :` (Excel
restrictions) — don't pre-truncate, the sidecar handles it.

### PPTX — slide deck

````markdown
```kronn-doc-data
{
  "format": "pptx",
  "slides": [
    { "title": "Q1 recap", "bullets": ["87 tickets done", "14 projects touched"] },
    { "title": "Next quarter", "content": "Focus on search v2 and mobile onboarding." }
  ]
}
```
````

Per slide: `title` + either `bullets` (preferred, array of strings) OR
`content` (plain paragraph, newlines split into bullet lines).

## Workflow — direct API call (fallback)

If the user is driving from a terminal or a script without the Kronn UI,
call the endpoints directly via Bash.

### PDF

```sh
curl -X POST http://127.0.0.1:${KRONN_BACKEND_PORT:-3140}/api/docs/pdf \
  -H "Content-Type: application/json" \
  -d '{
    "discussion_id": "<the current discussion id>",
    "html": "<your full HTML here>",
    "filename": "jira-annual-report",
    "page_size": "A4"
  }'
```

Response:
```json
{
  "success": true,
  "data": {
    "path": "/home/user/.kronn/generated/<disc>/jira-annual-report-ab12cd34.pdf",
    "download_url": "/api/docs/file/<disc>/jira-annual-report-ab12cd34.pdf",
    "size_bytes": 48213
  }
}
```

Show the `download_url` to the user as a relative link — the UI resolves
it. **Never fabricate** filenames or paths: return exactly what the API
gave you.

### DOCX / XLSX / CSV / PPTX

Same pattern, different body:

```sh
# DOCX — same HTML as PDF
curl -X POST .../api/docs/docx -d '{"discussion_id":"...","html":"..."}'

# XLSX
curl -X POST .../api/docs/xlsx -d '{"discussion_id":"...","sheets":[{"name":"S1","rows":[["A","B"],[1,2]]}]}'

# CSV
curl -X POST .../api/docs/csv -d '{"discussion_id":"...","rows":[["A","B"],[1,2]]}'

# PPTX
curl -X POST .../api/docs/pptx -d '{"discussion_id":"...","slides":[{"title":"T","bullets":["a","b"]}]}'
```

## Input formats by endpoint

| Endpoint | Body shape                                                   | Example use                       |
|----------|--------------------------------------------------------------|-----------------------------------|
| `/pdf`   | `{discussion_id, html, filename?, page_size?}`               | Report, invoice, formatted text   |
| `/docx`  | `{discussion_id, html, filename?}`                           | Word doc — same HTML as PDF       |
| `/xlsx`  | `{discussion_id, sheets: [{name, rows}], filename?}`         | Tabular data                      |
| `/csv`   | `{discussion_id, rows, delimiter?, filename?}`               | Flat dump                         |
| `/pptx`  | `{discussion_id, slides: [{title, content?, bullets?}], filename?}` | Presentation               |

## Tips for good output

- **HTML size**: WeasyPrint handles documents of hundreds of pages fine.
  If the result is massive (500+ rows), consider chunking the report
  into sections or breaking the page via CSS `page-break-before`.
- **Fonts**: stick to system fonts (Arial, Helvetica, Times, Georgia,
  "Courier New") — WeasyPrint can't resolve custom web fonts without
  the user's network access being allowed.
- **Images**: inline as base64 (`<img src="data:image/png;base64,...">`)
  or use the `base_url` hint (the Kronn backend sets it automatically
  to the discussion's working dir when you provide an HTML with
  relative `<img src="./chart.png">`).
- **Page size**: default A4 portrait. Pass `"page_size": "Letter"` or
  `"page_size": "A4 landscape"` when the content needs it.

## If something fails

The sidecar's error messages are relayed verbatim. Common cases:

- **"weasyprint not installed"** → operator ran setup before installing
  system deps. Tell the user to run the apt/brew commands from
  `make docs-setup` output.
- **"Sidecar request failed"** → the sidecar crashed or was killed.
  Restart Kronn to respawn it.
- **"PDF rendering failed: unsupported font"** → your HTML references a
  font the system doesn't have. Switch to a system-safe font.

You sign every generated document in the chat with a line like
*"📄 Generated via Kronn Docs — `jira-annual-report.pdf`"* so the
user knows which file to look for.
