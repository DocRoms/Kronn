# Audit prompt sources

Static audit instructions live in `backend/src/api/audit/prompts/` as Markdown
files embedded at compile time with `include_str!`. Rust still defines step
ordering, target files, source patterns, and dynamic prompt assembly. The
anti-hallucination section remains shared with the bootstrap template through
the existing synchronization test.
[src: file: backend/src/api/audit/mod.rs:185#PROMPT_PREAMBLE]
[src: file: backend/src/api/audit/mod.rs:210#ANTI_HALLU_SECTION_BODY]
[src: file: backend/src/api/audit/mod.rs:232#ANALYSIS_STEPS]

The extraction preserves prompt bytes, including whitespace. Validation-card
instructions and briefing prompts retain their three existing locales; Rust
still substitutes the briefing values at the call site.
[src: file: backend/src/api/audit/helpers.rs:704-720#validation_card_protocol]
[src: file: backend/src/api/audit/helpers.rs:1094-1114#build_briefing_review_prompt]
[src: file: backend/src/api/audit/helpers.rs:1117-1125#build_briefing_legacy_prompt]

The Drift prompt asks the agent to report checksum mismatches without rewriting
documentation. Its title no longer describes an unfinished placeholder; this
prompt is separate from the deterministic drift API.
[src: file: backend/src/api/audit/prompts/drift.md:1-3]

Prompt contract tests continue to exercise the embedded strings rather than
separate copies of the Markdown content.
[src: file: backend/src/api/audit/mod.rs:952-964#drift_kind_consumes_checksums]
