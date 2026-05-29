# Anti-hallucination — Playwright E2E scenario script

> QA companion to the deterministic backend matrix in
> `backend/src/core/anti_halluc.rs` (`mod tests::scenario_matrix`).
> This file is the **live** counterpart: how to drive a real agent on a real
> discussion to *try* to elicit each of the 6 anti-hallu pill states, and an
> honest note on which states can be forced reliably and which can't.

## Scope & honesty disclaimer (read first)

The backend matrix is deterministic because it calls `analyze`/`analyze_roots`
on fixed strings. **A live LLM is not deterministic.** The linter is a pure
function of the *agent's text output*, so to force a pill state E2E you must get
the agent to emit specific text — and a well-behaved modern agent will often
*refuse* to emit exactly the wrong thing we need (that refusal IS the
anti-hallu directive P1 working as designed).

Two layers matter:

1. **P1 (preamble)** — injected into the system prompt whenever mode ≠ `off`. It
   *discourages* the agent from emitting unsourced or fabricated citations.
2. **P2 (linter)** — runs on the finalized message text and produces the pill.

So the cleanest, most reliable way to E2E-test the *pill rendering + counts* is
to **quote the exact text we want analyzed back to the agent** and ask it to
echo it verbatim inside its reply (e.g. "reply with exactly this sentence, no
commentary"). That removes the LLM nondeterminism from the linter input while
still exercising the full live path: runner → finalize_lint_report → DB →
WebSocket → pill render. Where a scenario *can't* be forced even with echo
(because the agent's own guardrails refuse), that is called out per-type.

### Target & preconditions

- **Project / discussion**: `DOCROMS_WEB` (the standing audit fixture).
- **Mode**: Settings → Sourcing & Anti-hallucination set to **`warn`** (pills
  visible, non-blocking). Verify the toggle before the run.
- **Agent**: an agent with the `kronn-internal` MCP and file access on
  `DOCROMS_WEB` (so file-ref citations have a real root to resolve against).
  Note: file refs resolve against the disc's effective working tree — for an
  Isolated disc that is the worktree first, then the project checkout.
- **A real file that exists** in `DOCROMS_WEB`: pick one live during the run
  via the file explorer or `ls`; the script below uses the placeholder
  `<REAL_FILE>` (e.g. `src/index.ts`) and `<REAL_FILE_LINES>` for its length.
- **PW selectors**: the pill is the anti-hallu chip on the agent message bubble;
  open its drawer to read per-source statuses. Confirm the live `data-testid`
  / role during the run — do not hardcode a stale selector here.

### PW driving pattern (per scenario)

```
1. browser_navigate → DOCROMS_WEB discussion URL
2. browser_snapshot → locate the message composer
3. browser_type → the scenario prompt (below)
4. submit, then browser_wait_for → agent message finalized (stream done)
5. browser_snapshot → assert the pill state on the new bubble
6. open the pill drawer → assert source rows / counts
7. browser_console_messages → optionally cross-check the `anti_halluc`
   telemetry log line (unsourced/fabricated/verified counts)
```

Use generic placeholder identities everywhere (`TestUser`, `PeerAlpha`) — never
real names.

---

## 1. VERIFIED (green)

**Goal**: a citation mechanically resolves → green pill, `verified_count ≥ 1`,
all other counts 0.

**Prompt (echo strategy, most reliable):**

> Reply with EXACTLY this one line and nothing else:
> `The entry point is [src: file: <REAL_FILE>:1].`

**Expected pill**: GREEN. Drawer shows one source, status *Verified*, detail
"file exists" / "lines 1-1 within N lines".

**Honesty**: Highly reliable. The only failure mode is the agent adding
commentary, but the green still appears as long as the marker survives. An even
more *natural* variant — ask the agent to "tell me where the entry point is and
cite the exact file:line" — also tends to go green because a competent agent
emits a backticked `` `<REAL_FILE>:1` `` inline anchor (niveau 1.5
auto-verify). This is the happy path the feature was built for, so it forces
easily.

---

## 2. UNSOURCED (amber)

**Goal**: a confident technical claim with a claim-cue and **no** anchor →
amber, `unsourced_count ≥ 1`, `fabricated_count == 0`.

**Prompt (echo strategy):**

> Reply with EXACTLY this one line and nothing else, no backticks, no file path:
> `The function returns the cached connection pool handle on every call.`

(FR variant: `La méthode renvoie systématiquement une connexion réutilisée.`)

**Expected pill**: AMBER (unsourced). Drawer / flagged span shows the sentence +
the cue that tripped it (`returns ` / `the function` / `renvoie`).

**Honesty**: Moderately reliable *via echo*. Forcing it from a free-form prompt
is harder than it looks — P1 actively pushes the agent to *add* an anchor or a
hedge, either of which suppresses the flag. So a natural "describe what this
function does" prompt may well come back **green or no-signal** instead of amber
(the agent cites a file, or hedges with "appears to"). That is the directive
working. The echo prompt removes that variance. Do not assert amber on a
free-form prompt.

---

## 3. FABRICATED (red)

**Goal**: a **FORMAL** `[src: …]` marker that fails verification → red,
`fabricated_count ≥ 1`.

**Prompt (echo strategy — required):**

> Reply with EXACTLY this one line and nothing else:
> `This is handled in [src: file: ghost_does_not_exist.rs:1].`

Variants that also go red (one each, to cover the four fabricated statuses):
- `[src: file: <REAL_FILE>:999999]` → OutOfBounds
- `[src: file: ../../../../etc/passwd:1]` → OutsideProject
- `[src: training-data: model prior]` → Rejected

**Expected pill**: RED. Drawer shows status NotFound / OutOfBounds /
OutsideProject / Rejected, `is_fabricated` true.

**Honesty — this is the important one**: FABRICATED red **cannot be forced by a
"natural" wrong citation**. Two hard constraints:

1. **It requires a FORMAL `[src:]` marker.** A merely *inline* wrong citation
   (e.g. the agent writes `` `ghost.rs:1` `` in prose) goes **soft-amber
   UNVERIFIED, not red** (see §4). Only the bracketed grammar escalates to red.
2. **A well-behaved agent will usually refuse to emit a `[src:]` it knows is
   false.** The P1 preamble explicitly says a citation pointing at a
   non-existent path is rejected as fabricated. Ask it to invent a fake
   `[src:]` and a careful agent may decline, hedge, or "correct" the path to a
   real one — which is the anti-hallu *succeeding*, and you get green/no-pill
   instead of red.

**Therefore**: the echo-verbatim prompt ("reply with EXACTLY this line") is the
only reliable way to get red live, and even then some agents wrap the literal in
a disclaimer. If the agent refuses to echo a knowingly-wrong `[src:]`, **log
that as a PASS for P1**, not a test failure for P2. The deterministic red
coverage lives in the backend matrix; the live test only proves the pill
*renders* red when the text contains a failing formal marker.

---

## 4. UNVERIFIED (soft amber, NEW)

**Goal**: a **non-resolving INLINE** backtick anchor → soft amber,
`unverified_count ≥ 1`, `fabricated_count == 0` (NOT escalated to red).

**Prompt (echo strategy):**

> Reply with EXACTLY this one line and nothing else:
> `` Check `src/ghost_missing_file.ts:1` for the handler. ``

(OutOfBounds variant: `` `<REAL_FILE>:999999` ``.)

**Expected pill**: SOFT AMBER (unverified). Drawer shows the source listed,
status *Unchecked*, detail contains "inline anchor (couldn't verify)".

**Honesty**: This is the *most likely-to-occur-naturally* failure in real life —
an agent cites a real-looking `` `path:line` `` inline that's a typo, a
cross-repo path, or a stale line number. So a free-form prompt ("which file
handles X? cite it inline with backticks") **can** produce it organically when
the agent guesses a path. But it's still nondeterministic (the agent may guess a
*correct* path → green, or refuse to guess → no-signal). Echo is the reliable
forcing method. Key assertion: confirm it is **soft amber, not red** — this is
the niveau-1.5 honesty contract (inline wrong ≠ fabricated).

---

## 5. NO SIGNAL (no pill)

**Goal**: plain prose with no claim cue, no anchor → no pill at all
(`has_signal == false`, `finalize_lint_report` returns `None`).

**Prompt (echo strategy):**

> Reply with EXACTLY this one line and nothing else:
> `Thanks, that all looks good to me — have a great week!`

(FR: `Voilà, c'est terminé pour aujourd'hui. Bonne soirée à tous.`)

**Expected**: **no pill** rendered on the bubble. No drawer.

**Honesty**: Very reliable. Smalltalk / acknowledgements reliably carry no cue
and no anchor. The only risk is an over-eager agent appending a technical
sentence; the echo prompt prevents that. This scenario doubles as the negative
control — if a pill appears here, the linter has a false-positive regression
worth filing.

---

## 6. UNCHECKED / non-vérifiable (no pill, but listed in drawer)

**Goal**: only soft/non-file tiers (`url`, `user-confirmed`, `inferred`,
`commit`, `hypothesis`) → these are *Unchecked*, NOT counted in
verified/fabricated/unverified → **no pill**, but the sources **are listed** in
the drawer for transparency.

**Prompt (echo strategy):**

> Reply with EXACTLY this one line and nothing else:
> `See the docs [src: url: https://example.test/x] and [src: user-confirmed 2026-01-01].`

(Other tiers to spot-check: `[src: inferred: derived from the trait bound]`,
`[src: commit: abc1234]`, `[src: hypothesis: cache invalidated on write]`.)

**Expected**: **no pill** (no green/amber/red), because nothing was
mechanically verified, fabricated, or left unsourced. BUT — and this is the
nuance — if a drawer is reachable for the message, the unchecked sources should
be visible there. Note the subtlety: with `has_signal == false`,
`finalize_lint_report` returns `None`, so **the report (and its source list)
is not stored at all** — meaning in the *current* implementation there is no
drawer to open for an unchecked-only message. The backend matrix asserts
`sources.len() == 2` on the in-memory report, but the finalize gate drops it.

**Honesty / DEFECT NOTE**: see the defect below — there is a real tension here
between "list unchecked sources for transparency" (the stated intent of type 6)
and "no-signal reports are dropped at finalize". This scenario is the live proof
of that tension. Forcing the *text* is reliable (echo); what the *UI shows* is
the thing to observe and confirm against product intent.

---

## Defect found during this QA pass

**`finalize_lint_report` drops the unchecked-source list (type-6 transparency
gap).** `LintReport::has_signal()` returns `false` for an
"only-unchecked-tiers" report (url/user/inferred/commit/hypothesis with no
verified/fabricated/unsourced/unverified). `finalize_lint_report` then returns
`None`, so the report — *including the `sources` vec the agent honestly
declared* — is never persisted. The type-6 spec says these sources "should be
listed in `sources` for drawer transparency", and at the `analyze` level they
ARE (asserted in `unchecked_url_and_user_no_signal_but_listed`). But end-to-end
they vanish at the finalize gate.

This is **not a bug in `analyze`/the counts** (those are correct, and the tests
pass). It is a product-intent gap in the *gate*: an agent that diligently cites
`[src: url: …]` / `[src: user-confirmed …]` gets the same "no pill, no drawer"
treatment as smalltalk, so the user can't see that sourcing discipline happened.
Per instructions I did **not** change `anti_halluc.rs` logic for this (it's a
design decision, not a clear logic error) — flagging it for product:

- Option A (no code change): accept it — "unchecked-only" is genuinely
  no-signal, drawer transparency only matters when there's already a pill.
- Option B: add a 4th, neutral pill state ("sourced, unverifiable") so
  `has_signal()` returns true when `sources` is non-empty even if all
  Unchecked, and the drawer becomes reachable. This would make type-6 E2E
  observable and matches the stated transparency intent.

Recommend product decide A vs B before relying on the §6 live scenario.
*(Resolved 2026-05-30 → Option B: neutral "unverifiable" pill + drawer group.)*

---

## Defect found during the deep re-pass (2026-05-31)

**Web-project source extensions (`.twig` / `.xlf`) were missing from the
allowlist → every Twig/XLIFF citation went unverified.** A 4-conversation
forensic re-pass (one linguistic expert per persona, reconciled against the
machine verdict) on a real **Symfony** project found the dominant gap: the two
`SOURCE_EXTS` lists in `anti_halluc.rs` (`contains_code_anchor` niveau-0 +
`looks_like_file_anchor` niveau-1.5) carried no `.twig` / `.html.twig` / `.xlf`
/ `.scss` — i.e. the *primary* files of a web project. So real citations like
`` `templates/pages/projets.html.twig:82` `` resolved to nothing (and the
sentences citing them read as *unsourced* → false positives too). The two lists
had also drifted (`.php` was in one only).

**Fixed**: unified into one shared `SOURCE_EXTS` const + added the web
extensions. No double-extension special case (`ends_with`, and `foo.html.twig`
ends with `.twig`). Proven on the real conversations: verified sources **19 →
35**, 0 false red, 8 non-resolving anchors (proposed/placeholder files)
correctly surfaced as soft-amber.

**Live scenario to add when convenient**: on a Symfony repo, ask the agent to
"locate the projects page template and cite the exact file:line" → expect a
GREEN pill on the `.html.twig` anchor (was silently dropped before the fix).

---

## Summary table

| # | State | Forcing method | Live reliability |
|---|-------|----------------|------------------|
| 1 | VERIFIED (green) | echo formal `[src:]` or natural inline anchor | High |
| 2 | UNSOURCED (amber) | echo claim sentence, no anchor | Medium (P1 fights it) |
| 3 | FABRICATED (red) | echo a FORMAL failing `[src:]` | Low natural / High via echo; agent may refuse (= P1 win) |
| 4 | UNVERIFIED (soft amber) | echo a wrong inline `` `path:line` `` | Medium-high; occurs naturally |
| 5 | NO SIGNAL | echo smalltalk | High (negative control) |
| 6 | UNCHECKED | echo url/user/inferred markers | Text forceable; **UI drop — see defect** |
