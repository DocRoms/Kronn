# UI text — glossary, French register, plurals

KT-1879 (design-system lot 1, decided before any label is rewritten). The
screens' texts change in the lots that touch each screen (KT-1880 to
KT-1886); this lot fixes the vocabulary, the register and the plural function,
and checks new labels against them. Pairs with
[design-tokens.md](design-tokens.md).

## Where it lives

- **Glossary:** `frontend/src/lib/i18n/glossary.json`, next to the locale
  files. One term per concept in en, fr, es and zh, the synonyms to avoid, the
  French register and two proposals for the homonym « principal ».
- **Baseline:** `frontend/src/lib/i18n/terms-baseline.json`: every label that
  broke a rule on 2026-11-21 (hash of its text + the rules it breaks) and the
  count per cluster.
- **Check:** `frontend/scripts/i18n-terms.mjs`, no dependency.
  `node frontend/scripts/i18n-terms.mjs` (or `pnpm run lint:terms` in
  `frontend/`), and the same check in vitest:
  `frontend/src/lib/__tests__/i18n-terms.test.ts`.
  `[src: file: frontend/scripts/i18n-terms.mjs:242#checkTerms]`
- **Plural:** `pluralForm()` in `frontend/src/lib/i18n/plural.ts`, read
  through `tPlural()` or `tp()` from `useT()`.
  `[src: file: frontend/src/lib/i18n/plural.ts:23#pluralForm]`
  `[src: file: frontend/src/lib/i18n.ts:112#tPlural]`

## Owner decisions

| Date | Decision |
|---|---|
| 2026-11-18 | French says « vous » everywhere. |
| 2026-11-18 | The tabs are named « Tâches » (en/es/zh: task, tarea, 任务) and « Réglages » (settings, ajustes, 设置). |
| 2026-11-18 | Quick API and Quick Exec are two types, API and CLI, of one family: quick actions (« actions rapides »). |
| 2026-11-20 | The agent that owns a discussion is the « agent principal » / main agent / agente principal / 主代理. « agent de discussion », « agent de la discussion » and their en/es/zh forms are synonyms the check reports. KT-1936 rewrote every label that used them; none is left in the baseline. |

Every other concept is marked `"decided": "proposed"`: its term is the most
used one in the count of 2026-11-21, open to the owner. Changing one is a
glossary edit followed by a baseline refresh (below).

## The homonym « principal » (to settle)

The word names two roles besides the main agent. The glossary proposes a
term for each; the labels are not rewritten in KT-1879 and the check does not
enforce these yet.

| Use | Keys | Proposal (en / fr / es / zh) |
|---|---|---|
| The agent that delegates a task to a worker and reviews its delivery | `orch.approveEvidence`, `wiz.roomIdHint` | reviewing agent / agent relecteur / agente revisor / 审查代理 |
| In a shadow comparison, the step's own agent against its shadows | `wf.shadow.*`, `wf.branchMap.principal` | step agent / agent de l’étape / agente del paso / 步骤代理 |

## What the check reports

- **A synonym** of a glossary term, as a whole word: a letter, digit, `_`,
  `-`, `/` or `.` next to it means a compound, a path or an identifier
  (`cross-repo`, `/Users/vous/Repos`, `config.json`). In zh only Latin
  characters count. Inline code, fenced code, `{{templates}}` and URLs are
  skipped.
- **A « tu » form** in French: tu, te, t’, toi, ton, ta, tes, tien(ne)(s).
  « le ton », « un ton », « même ton »… (the tone) are allowed. Imperatives
  such as « Décris » are not detected: only the pronouns and possessives.
- **Allowed phrases:** a concept's `allow` list holds literal, case-sensitive
  phrases; a match that overlaps one is not reported. The tab names
  « Automatisation », « Automation », « Automatización » and « serveur MCP »,
  « MCP kronn-internal »… are there, and « CLI 会话 » (a CLI session, not a
  discussion; KT-1936).
  `[src: file: frontend/scripts/i18n-terms.mjs:128#findViolations]`

It fails only on a label that is **new**, **touched** (its text changed and it
still breaks a rule) or that breaks a rule its baseline entry does not list
(the glossary grew). A recorded label left as it is passes.

## Writing a label

- Use the glossary term, and « vous » in French.
- If the flagged word means something else in this label, add the exact
  phrase to the concept's `allow` list in the same change.
- Rewriting a recorded label: fix it, then
  `node frontend/scripts/i18n-terms.mjs --update-baseline`. That refresh only
  drops labels that no longer break a rule; it never records a new one.
- After a glossary change (a new synonym, a new concept), record the labels it
  now reports with `--update-baseline --accept-new`, and say so in the PR: the
  baseline diff lists them one per line.
  `[src: file: frontend/scripts/i18n-terms.mjs:218#buildBaseline]`

## Plurals

One function decides the form for every locale; a countable label stores two
forms, `<key>.one` and `<key>.other`, in **every** locale (zh too, where
`.one` is never read, so that the key parity check holds):

```ts
// fr.ts: 'projects.left.one': '{0} projet restant', 'projects.left.other': '{0} projets restants'
const { tp } = useT();
tp('projects.left', count);          // count is {0}; further arguments are {1}, {2}…
```

| Locale | `one` | `other` |
|---|---|---|
| en, es | exactly 1 | 0, 2, 1.5, … |
| fr | 0 ≤ n < 2 (0, 1, 1.5) | 2, … |
| zh | never | always |

These are the CLDR cardinal rules; CLDR's « many » (round millions in fr and
es) takes the ordinary plural. `lint-i18n.mjs` counts `tp('key', …)` as a use
of `key.one` and `key.other` and fails if either is missing.

It replaces the three mechanisms found in the locales on 2026-11-21: `(s)` in
the text (« {0} image(s) »), a `{1}` argument the caller fills with `'s'`
(« {0} workflow{1} en cours »), and key pairs chosen in the component
(`…Plural`, `…One` / `…Many` with `=== 1` or `> 1`). Each moves to `tp()`
with the screen it belongs to.
