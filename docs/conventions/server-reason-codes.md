# Reasons the server writes: a code for the interface, English for agents (KT-2003)

A reason Kronn itself writes for an unavailable agent or a refused model
(catalogue verdicts, preflight refusals) reaches the API twice: the English
sentence, unchanged, for agents and logs, and a `CatalogReasonCode` with its
parameters next to it (`detail_code`, `notice_code`, `unavailable_code`,
`last_error_code`). The interface writes the sentence from the four locale
files. See also [ui-text.md](ui-text.md).

- The codes: `[src: file: backend/src/models/model_catalog.rs:86#CatalogReasonCode]`
- The refresh log and the catalogue rows store only the English sentence; the
  code is read back from it when the row is read, so a sentence Kronn writes
  into the DB must be a constant the reader compares with:
  `[src: file: backend/src/core/model_catalog/mod.rs:469#refresh_error_code]`,
  `[src: file: backend/src/core/model_catalog/mod.rs:522#entry_reason_code]`
- The fixed sentences a discovery module writes (CLI missing, sign-in needed,
  unreadable catalogue, failed discovery, an HTTP status) are `pub(crate)`
  constants of that module, recognised by kind with the agent's Settings
  label; a sentence that embeds an OS error, a CLI's stderr or an ACP
  transport detail keeps its quote.
  `[src: file: backend/src/core/model_catalog/mod.rs:492#discovery_error_code]`,
  `[src: file: backend/src/core/model_catalog/claude_discovery.rs:37#CLI_NOT_INSTALLED]`
- The interface: `serverReasonText` writes a known code, falls back to the
  English sentence for an unknown one, and quotes a text without a code (a
  CLI's stderr, a provider's answer) under the sentence of its
  `ModelUnavailableReason` (`serverReason.lead.*`).
  `[src: file: frontend/src/lib/serverReason.ts:86#serverReasonText]`
- Every code needs a sentence: the `SENTENCES` table is typed on the code
  union, so a new backend code does not type-check until it is written.
  `[src: file: frontend/src/lib/serverReason.ts:50#SENTENCES]`
- A menu path inside a reason is built from the navigation's own keys
  (`nav.config`, `config.agents`, `config.fullAccessName`), never written in
  the sentence, so a tab rename follows. « Full access required » is one
  sentence for the server's refusal and every pre-launch warning (Dashboard,
  workflows, new discussion, composer), and the link that opens the Agents
  section is built the same way.
  `[src: file: frontend/src/lib/serverReason.ts:28#fullAccessSettingsPath]`,
  `[src: file: frontend/src/lib/serverReason.ts:36#fullAccessRequiredText]`,
  `[src: file: frontend/src/lib/serverReason.ts:40#agentsSettingsLinkLabel]`

## Adding a reason

1. Add the variant to `CatalogReasonCode`, set it where the English sentence
   is written (or recognise the stored sentence), and test it with a name that
   contains `reason_code` (`cargo test --lib reason_code`).
2. Regenerate `frontend/src/types/generated.ts`, then write the sentence in
   `SENTENCES` and its key in the four locales.

A text from a third party gets no code: it is a `detail` parameter or the
English `detail` itself, quoted as received.
