# Inconsistencies & tech debt (index)

> Entry point: `ai/index.md`. Details: `ai/tech-debt/<ID>.md`.

## Purpose
- A shared list (human + AI readable) of **known inconsistencies** and **things that should be improved**.
- This file is **track-only** — it exists to prevent large sweeping changes by AI and to help create tickets.
- **Details are in individual files** under `ai/tech-debt/`. Only load a detail file when working on that specific topic.

## How to add an entry
1. Create `ai/tech-debt/TD-YYYYMMDD-short-slug.md` using the template below.
2. Add a one-line summary to the list in this file.

## Entry template (for detail files)
- **ID**: TD-YYYYMMDD-short-slug
- **Area**: (e.g. Backend | Frontend | CI | Config | Docs | Dependencies | Infrastructure)
- **Severity**: Critical | High | Medium | Low
- **Problem (fact)**: ...
- **Why we can't fix now (constraint)**: ...
- **Impact**: dev friction | test fragility | perf | security | correctness | docs
- **Where (pointers)**: files/paths/targets
- **Suggested direction (non-binding)**: ...
- **Next step**: ticket link or 'create ticket'

## Outdated prerequisites & deprecated dependencies

| Component | Current version | Status | Risk |
|-----------|----------------|--------|------|
| {{COMPONENT}} | {{VERSION}} | {{STATUS}} | {{RISK}} |

## Current list

| ID | Problem | Area | Severity |
|----|---------|------|----------|
| {{ID}} | {{PROBLEM}} | {{AREA}} | {{SEVERITY}} |
