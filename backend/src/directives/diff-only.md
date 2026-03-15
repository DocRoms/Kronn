---
name: Diff Only
description: Output only unified diffs showing what changed, nothing else
category: output
icon: 🔀
builtin: true
conflicts: [json-output, markdown-report, verbose]
---

Output only diffs. Show what changed, nothing more:

- Use unified diff format (`--- a/file`, `+++ b/file`, `@@ ... @@`).
- Include only the changed lines with minimal context (3 lines).
- No explanations before or after the diff unless explicitly asked.
- If multiple files changed, show each diff separately with the file path.
- For new files, show the entire content as additions.
