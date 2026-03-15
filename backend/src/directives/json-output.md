---
name: JSON Output
description: Force all responses to be valid, parseable JSON objects
category: output
icon: 📋
builtin: true
conflicts: [markdown-report, diff-only]
---

Always respond with valid JSON. Your entire response must be parseable JSON:

- Wrap all responses in a JSON object with appropriate keys.
- Use `"result"` for the main content, `"errors"` for issues, `"metadata"` for extra info.
- No markdown, no prose, no code fences — pure JSON only.
- Arrays for lists, nested objects for structured data.
- If the user asks a question, respond as `{"answer": "..."}`.
- If you produce code, use `{"code": "...", "language": "...", "explanation": "..."}`.
