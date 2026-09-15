# Changelog

All notable changes to Kronn are documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Release notes for 0.9.3 and earlier are available in the
[legacy archive](docs/releases/CHANGELOG-legacy.md).

---

## [Unreleased]

### Added

- Installed CLI versions are compared with current stable releases from their
  official sources. RTK and ccusage have separate checks and diagnostics;
  checking never installs an update. Cached results and an explicit recheck
  keep discovery bounded instead of hard-coding the next version to expect.
- Claude model pickers use the installed runtime's account-specific catalogue,
  including newly exposed model identifiers, and share refreshed results.
  A transient discovery failure preserves known available choices; an actual
  authentication or availability failure is still reported.
- Claude and Codex tiers can pair a model with an optional reasoning effort
  advertised by that model. An execution override wins over a same-model tier
  preset; leaving both empty sends no effort flag and keeps the CLI default.
  Changing models clears an incompatible preset in the same save. A Quick
  Prompt launch keeps its explicit effort independently of later prompt edits
  or deletion, bound to that discussion's agent, tier and resolved model.
- **Important message** opens a plain-text form above the ordinary composer:
  Information, Decision or Blocker, with an optional link to an existing task
  in this discussion's plan. No JSON to write and no permanent key field in the
  normal composer. Publishing never creates, edits or completes a task; the
  card's link opens that task in the plan. The ordinary draft stays independent.
  Form drafts survive closing and navigation in memory, not a browser reload.
  An uncertain send keeps its original identity for reconciliation instead of
  blindly publishing again.
- Important messages are persisted objects, not formatting. The structured
  `kronn-important` contract also supports a scope change, a DoD waiver, a blocking
  alert, an action needed from a human, an accepted delivery — and Kronn keeps a
  card the discussion can filter, count and step through, instead of a bold line
  that scrolls away. A worker's delivery report stays a delivery report: when a
  fact inside one matters, a separate card points at it. A replay after a
  restart collapses onto the fact it already recorded rather than doubling it.
- Publishing a card asks who you are. It takes a credential enrolled in
  Settings and a single-use proof bound to the room and to the exact text, so a
  captured request cannot be replayed or aimed somewhere else. A worker
  publishes none while it is working, holding a valid credential or not — it
  reports through its delivery. Caller-authored cards require an enrolled
  credential; without accepted authority the ordinary message still posts and
  the response explains the refused card. Kronn's own orchestration steering
  events use their backend authority and do not require a human credential.
- The publication bootstrap can be rotated, and recovered when it is lost.
  Rotating it from Settings writes the replacement to the operator's private
  file and answers with the path — the secret travels over no wire, and the
  screen locks back because the authority just used is gone. An operator who
  lost it leaves a `recover-admin-secret` file in the Kronn data directory and
  restarts. Credentials already enrolled keep working through both.
- An agent can put a decision to the human and stop, without the question
  scrolling away. A `kronn-question` block becomes a card that stays pinned
  while it waits: the question, what it blocks, the options with their
  consequences, the agent's recommendation — marked, never pre-selected — and
  free text, because the right answer is regularly one nobody listed. The
  answer is a durable message, so whoever picks the work up later reads the
  decision instead of asking again. A malformed block says which field refused
  it rather than vanishing in silence.

- A discussion can be opened for a media generation alone. New discussion
  offers `Generate media` when a compatible model is configured; the room is an
  ordinary one with no agent, and nothing is generated until the form is
  explicitly submitted.

- An image in the assets carousel can become another image or a video. The
  actions appear only for what the configured models can actually do, and the
  source image is carried into the generation form rather than re-attached by
  hand.

- HTML source files preview in place. Projects > Code switches between the
  source and a sandboxed static render — no scripts, no navigation, no network
  — and Live Pages revision diffs offer the same before/after view.

- A provider whose quota escalation was never closed can be re-armed. Tasks
  that reached a terminal state stop holding the lock, and an audited manual
  re-arm releases the rest without replaying any old work.

- A reassigned worker can deliver again after a timeout or unavailability
  interrupted an unreviewed delivery. The next attempt gets fresh review
  identities; an already occupied message ID no longer rolls the delivery
  back. Earlier messages stay intact and the new request names the current
  commit to review.

- Quick Prompts read `{{env.NAME}}` like every other placeholder, and their
  editor offers the project's own environment names rather than a blank field.

- The discussion plan filters on what is running. `Focus / In progress / All`,
  with an indicator on tasks that are actually working, and the choice is
  remembered.

- Kronn accepts support through Ko-fi, from the GitHub funding metadata and a
  card under the version in Settings.

- Search can be told where to look. A term that appears in one discussion's
  title and in twenty transcripts used to drown the room actually named after
  it, and no amount of ranking fixes that — the reader wants to exclude, not to
  re-sort. So the scope is a filter, title / content / both, and it is
  remembered between visits. Under "both", a title match now comes before a
  content match: someone searching for a name is looking for a place, not for
  an occurrence. Both HTTP search endpoints gained the same scope; the MCP
  `disc_search` tool does not offer it yet, so an agent's search stays unscoped.
- An approved delegation now publishes its report. The derivation and the
  store existed but nothing called them, so an accepted delivery produced
  nothing at all. Both approval paths publish it — the ordinary one, and a
  replayed approval that repairs a crash between the approval and its message.
  The record and its message are written in a single transaction, so a retry
  cannot leave an accepted delivery with no report; a record found without its
  message gets one, rendered from the stored payload rather than the caller's.
  A report is produced even when parts of the manifest are unreadable — a
  degraded report says what is missing, which is more use than no report. The
  worker's duration counts from its own assignment, so earlier review rounds
  are not charged to its attempt.
- Workflows, quick prompts, quick APIs and quick execs can be deleted from
  their own row — armed by a first click, done by a second, never before the
  server confirmed — and several at once from the sidebar selection. While
  armed, the control says what goes with it: the past runs a workflow takes
  along (their discussions stay), the workflow steps a prompt or an API is
  used by (they fail on their next run), or that nothing else is touched. A
  count that cannot be read says so, instead of reading as nothing.
- A clip's last frame can be kept as an asset of its own, with a link back to
  the clip it came from. It is decoded in the browser, by the player already
  showing the clip, and runs only from a click in that viewer — there is no
  headless path, so no agent can ask for it through MCP. It works wherever the
  interface is served, Docker included.
- Image and video generation on HTTP connections (LiteLLM, NVIDIA, OpenRouter).
  Media models are configured as their own slots on a connection — modalities,
  not quality tiers — so a text step can never select "tier Image". A
  generation is launched from a discussion's Assets tab or by an agent through
  MCP (`media_generate`, `media_job_status`); each launch gets its own bubble at
  its chronological place in the transcript, which turns on the spot from
  pending into the finished media — or into a stated failure — and comes back
  the same way after a reload. The launcher is free again the moment the job is
  accepted, so a video and two images can progress in parallel without mixing
  up their states. Videos play inline with native
  Picture-in-Picture, and opening any asset browses every image and clip of the
  discussion in one carousel. Media spend is its OWN counter, reported per
  generation from the provider's billed figure, never recomputed from a
  published rate. The estimate shown before sending comes from past billed
  generations, and reads as unknown — never as free — when there is none.
  A finished generation shows the media itself inside its bubble, with the
  price in the corner and a single way out — the asset in the carousel; the raw
  job JSON folds away behind a "Details" toggle, and the only duration on
  screen is the one of a produced clip. A video's soundtrack is now a stated
  choice on both surfaces — a checkbox in the form and `generate_audio` on the
  MCP tool, checked/true by default, which is what the providers were already
  doing silently. That silence had a clip refused for audio copyright with
  nothing in the request to explain it. The launcher's durations, resolutions,
  ratios and audio switch now come from the provider's own catalogue instead of
  a hard-coded list that offered choices the model rejects, and a video can
  start from — or end on — an image the discussion already holds, picked in the
  form or named by an agent through MCP. That image is referenced by id and
  travels inline: no local path, URL or credential ever leaves Kronn.
  An asset can finally be deleted, from the viewer, in two steps — the control
  removes bytes from disk and sits next to "close", so it arms before it acts,
  the viewer closes rather than silently landing on the neighbouring media, and
  a server refusal leaves the file exactly where it was.
  An image generation can now be drawn from SEVERAL pictures of the discussion,
  up to the number each model advertises — from 1 to 16 across the catalogue,
  so nothing is assumed — and the bubble opens every one of them. Asking for a
  frame on an image, or for several images on one frame, is refused instead of
  being quietly reduced to something else and billed for it.
  A clip's last picture can be kept as a file of the discussion in one click,
  and reused straight away as the starting image of the next generation — no
  download, no re-upload, no dependency to install. It is decoded by the
  viewer's own player, which reads these clips where the backend cannot, and an
  extraction that decoded nothing says so rather than attaching a black
  rectangle. A picture too narrow for the provider blocks the launch before it
  is billed.

- Discussions now report their storage weight, split by what a cleanup could
  actually reclaim: attachment bytes held on disk, extracted document text, and
  message content. The sidebar shows a green / amber / red indicator whose
  detail panel breaks the three masses down and states how much is reclaimable
  without losing any conversation. The indicator is configurable from Settings
  (`[server.discussion_weight]`: `enabled`, `amber_bytes`, `red_bytes`) and
  disabling it removes the queries entirely, not just the badge. Weights are
  served by a bounded batch endpoint — it never scans every discussion.

- Codex and Claude Code run through the same create/resume/stream/cancel/
  close ACP contract as the native ACP agents, and that contract is the normal
  path rather than an opt-in. The per-agent environment variables
  (`KRONN_ACP_ADAPTER_CODEX` / `KRONN_ACP_ADAPTER_CLAUDE`) changed meaning
  accordingly: unset selects the adapter, and returning to the direct CLI now
  takes an explicit `0` or `false` — a deliberate, observable fallback instead
  of the default. Task workers take the same route, with the narrow policy the
  direct builder already applied carried over whole: their own worktree, no
  inherited user configuration, a short tool allowlist, a fresh session, and no
  resume of a room's conversation. A shared, scoped, audited permission broker
  denies filesystem/terminal requests, unbound sessions, out-of-project paths
  and unauthorized MCP server/tools by default, and a worker never receives the
  room bridge an ordinary discussion gets. Project MCP servers are never
  inlined with credentials, prompts travel on stdin, and normal discussions
  persist the native Claude/Codex conversation id so a backend restart resumes
  the same CLI session without reusing it across projects. See
  `docs/operations/acp-adapters.md`.

- A dedicated `http_transport` module now owns the seam between LiteLLM,
  NVIDIA and named Custom connections and the shared OpenAI-compatible chat
  codec, kept deliberately separate from the ACP boundary (`docs/design/
  adr-004-http-transport.md`). The OpenAI Chat codec selection is an explicit,
  single decision point; a model the catalog marks image/video-only is refused
  before dispatch with a diagnostic rather than sent to the wrong endpoint, on
  every path that applies the guard. Two do not yet: multi-agent orchestration
  never calls it, and Workflow Agent steps call it without naming the
  connection the step will actually use, so the model is judged against the
  agent's own catalogue instead of that connection's.
  Discussions persist a sticky named connection (`discussions.connection_id`)
  so an ordinary reply with no explicit `@mention` keeps resolving through the
  same connection instead of losing it — previously only the very first
  message of a Custom-connection discussion reliably carried its target.
  Compare's AI judge/prompt-improver launch and multi-agent orchestration
  debates can now address a specific named connection instead of only a bare
  agent type. The connection-mismatch validation Quick Prompts apply does not
  yet cover the orchestration path. See `docs/operations/http-transport.md`.

### Changed

- Projects > Code fetches one folder when it is opened, instead of the whole
  repository up front. Opening the tab went from 4.19 s to about 0.10 s on this
  repository, and the cost no longer grows with it. The 10 000-file ceiling that
  used to cut the tree — silently, with the missing files indistinguishable from
  files that do not exist — is gone; what remains bounds a single answer and
  says so when it is reached.

- A `[src: …]` citation reads as a chip rather than as notation. The file's name
  and lines instead of its whole path, a sha cut to seven characters, a URL by
  its host — with the exact reference on the chip, because a citation exists to
  be checked. It carries the verdict the backend already reached: verified,
  suspect, or honestly unknown.

- The panel strip floats over the conversation instead of holding a column.
  Closed, it reserved its width down the whole height for two buttons sitting at
  the top of it; only the message-search bar now steps aside for it.

- The Kronn mark is drawn rather than resampled, at every size it appears —
  browser tab, desktop icon, app header, share card — and the startup screen
  turns it slowly instead of showing a generic spinner.

- The model catalogue is one sorted table instead of ten stacked lists. A real
  install carries 637 models across ten sources — 502 from a single router — and
  finding one meant scrolling past the other 636. They are now one alphabetical
  table, sortable by name or by which source they belong to, invertible, with a
  search that matches the exact model id as well as the displayed name, and a
  click on a source narrows to it. The sources keep their re-check control on
  one line each rather than one card each.
- Adding a model by hand moved to the foot of that block. Measured on a real
  install: 624 of the 637 models were detected, ten migrated from an older
  configuration, three cached, and none had ever been added by hand — so it is
  not the everyday act the header made it look like. It stays, because a target
  whose detection returns nothing has no other way to name a model.

- Settings names the three ways of reaching a model. Config > Agents listed
  seven full-width CLI cards, then Ollama as a special case inside the same
  loop, then the external connections in a framed box of their own — one
  undifferentiated list, 1 544px of it. There are three framed zones now, CLI /
  Local / External API, built the same way so they read as three of a kind, and
  the model catalogue moved below them: a catalogue is what the modes draw
  from, not a fourth way of reaching a model.
- A CLI agent's card shows which model each tier will actually run, the way an
  external connection already did — economy, standard, reasoning, with the
  configured override or the built-in fallback, never a bare "default" that
  hides what runs. Configuring them stays behind the edit control, which now
  sits next to delete as an icon pair, so a CLI agent and an API connection
  offer the same two actions in the same place.

- The agent cards sit in two independent columns. Laid out as a grid, a row
  was as tall as its tallest card, so a short one left a hole beside a tall one
  — and expanding either pushed both columns down, moving cards the reader was
  not looking at. Each column is its own stack now: a card only ever moves the
  cards under it, in its own column. Below the breakpoint the two collapse back
  into one list in the original order.

- An agent's card says its name once. The title and the mention-colour chip
  beside it both carried it; the title is the colour control now, so picking a
  colour happens on the name that colour applies to. What only matters while
  configuring — the full-access switch, the API keys, the model tiers — folds
  behind a "Configure" button, and a card at half the width stacks its header
  instead of splitting it into a left and a right side.

- The discussion header reads as two lines instead of three. Inviting a peer
  now sits at the right of the title's own line, where an action on the whole
  discussion belongs, and the participant chips moved down to share the second
  line with the control that decides whether the discussion answers by itself —
  first position, since it changes what everything beside it means. What the
  discussion IS rather than what it does — its project, its cost, its worktrees
  — folds behind a "details" button at the right of that second line, and the
  fold is remembered, so anyone who wants those figures permanently opens the
  row once.
- The panel strip lines up with the message-search bar beside it in both
  states — open and closed — its delete keeps the red it had in the header, and
  search moved to the right of the control that opens the strip. That control
  now shows an active state, so the strip says which of its buttons put the
  panel on screen.
- A discussion's title no longer vanishes on a narrow screen. The id pill and
  the session binding took 230px of the title's own line between them, and the
  title was the one element allowed to give way, so below 768px it was given
  none at all. Those two move into the details fold there, and the title keeps
  the width it needs.
- A discussion's header stopped acting on the discussion. Search, export and
  delete joined the panels in the strip that sits above the panel column, so
  the header only describes what the discussion IS — its title, its agent, its
  tier, its counters. Message search opens level with that strip rather than a
  few banners lower, because the control and its field belong to the same row.
- The attached-runs list is gone from the transcript. It repeated what the
  Automations page already shows, and cost a slice of the conversation's height
  to do it; a launched action is visible as its own card in the thread.
- A discussion's panels — plan, assets, code, terminal, settings and message
  search — left the header row. It also carries the title, the agent, the tier
  and the counters, and every version narrowed it further. One control now
  opens the panel column, landing on whichever panel was last read, and it
  lives in that column rather than in the header: a control that opens a panel
  has to sit against it, and a row of counters came between the two. Moving
  between panels happens in the same strip, which widens into icons once a
  panel is open. Each panel already draws its own header, so listing those same
  icons up in the discussion header showed every one of them twice. Clicking
  the panel already open closes it, as the header buttons did.
- A turn no longer retells the whole discussion. Every message re-narrated the
  entire history to a brand-new process: on the four longest discussions in a
  real database, 1 288 Claude Code turns sent **436 million characters** where
  the same turns need **1.9 million** — an average of 338 984 characters per
  turn against 1 507. Claude Code already holds that conversation, so the turn
  now resumes it and carries only what the agent has not seen. Counting the
  preamble that repeats either way, a turn shrinks by a factor of 17 to 97
  depending on how much context the discussion mounts. Five things keep it
  honest: the delta is everything since the agent's last turn, never just the
  newest message, because in a room the human or another agent writes in
  between; the marker is a message id, so a history that was edited or pruned
  simply fails to match and the full prompt goes out, where a numeric cursor
  would have sent the wrong slice; the cursor only advances once the reply is
  stored, so an interrupted turn is replayed rather than skipped; a dead
  conversation id would fail the turn outright, so the CLI's session store is
  checked first and a miss means full prompt; and resume never travels without
  its delta, nor a full prompt with a resume. Task workers never resume — a
  worker opens on a fresh worktree, and a room's history is not its business.
  The figures above are measured on message volume, not end-to-end latency.

- An agent's prompt no longer carries every MCP server's documentation. It
  concatenated all of `docs/operations/mcp-servers/*.md` in full on every spawn
  — 69 196 bytes on the recorded Kronn benchmark, 45 465 for `kronn-internal.md`
  alone — whatever the discussion was about. It now carries the server listing,
  the `mcp__<server>__<tool>` convention, a pointer to `tool_manual` for the
  Kronn tools, and where to read a server's own notes if the agent is going to
  use it: 907 bytes, 77 times smaller, about 17 000 tokens saved per turn. This
  costs no latency either way — measured at 4.40 s against 4.44 s bare — so it
  is a cost fix, not a speed one. Only CLI agents ever received this block, and
  they have a filesystem: pointing at the file loses nothing and defers the
  reading to the turn that needs it. Discussions without a project already
  worked this way, and the two paths now agree.

- Automatic conversation summaries are gone. They fired after every reply past a
  per-agent threshold, and had been dead in practice: the global default was
  `Off`, acting as a master kill-switch, so a discussion displaying `Auto` never
  summarised — a strategy shown in the interface that could not apply. Removed
  rather than repaired, because every runtime can now read the thread back
  itself (`disc_meta` and `disc_get_message` over MCP, or a declared tool),
  which is cheaper and more
  precise than a summary produced in advance for a need nobody expressed. The
  `Auto` strategy no longer exists; rows written before 0.13.0 read back as
  `OnDemand`, which keeps `disc_summarize` and the summarise action working for
  an agent or a human reopening a long room. When history is trimmed, the notice
  now tells the agent to reread the thread first, and to ask the user only if
  the answer is not there.

### Fixed

- Projects > Code listed nothing under folders that came late in the alphabet.
  pnpm's content-addressable store is not a name the hand-written skip list
  knew, and it filled the 10 000-file budget on its own, so the walk stopped
  before reaching `site/`. Git already knows what is generated: the walk asks it
  once and does not enter what it ignores, which takes this repository from over
  10 000 files to 2 088.

- A folder that had not loaded yet was drawn exactly like an empty one. It
  opened onto nothing and its contents turned up seconds later; it now says it
  is loading, and says so differently when the request failed.

- A directory reached through a symlink could serve files from outside the
  project. Symlinked children were skipped, but the directory the caller named
  never was — precisely because it was the one they chose.

- The global search re-ran the same term without noticing the scope had changed.

- A project's Docker link opens the port the container actually published.
  Compose files routinely name that port through a variable, so several
  projects on one machine take turns on 443 and the rest land on 8443 or 9443
  — but Kronn read the compose file's default and sent the link to 443
  regardless. It reached whichever project holds that port, which answers with
  an error from an unrelated application: nothing says you are on the wrong
  project, so the bug gets hunted in the wrong place. The running container's
  own publisher is now the authority, the port is still omitted when it is the
  scheme's default, each published host keeps its own, and an endpoint nothing
  is listening on is shown as unreachable rather than offered as a link.

- OpenCode answers in the room again. Every chunk it streams carries its text
  as a single content object, a shape the ACP reader handled neither as a
  string nor as an array — so the whole reply fell through, the turn was
  recorded as failed with no visible output, and the message offered two
  guesses that were both wrong. Its private reasoning, which arrives the same
  way, is read and deliberately not shown: it is a scratchpad, and folding it
  into the answer would leak it. The turn's tokens are read from the response
  too, where ACP puts them, instead of being dropped with it — a reply that
  cost 7 946 tokens was recorded as costing nothing.

- The frontend lint gate is green again, by fixing what it flagged rather than
  by raising its ceiling. The release had added twenty-six warnings to a budget
  that had four left; all twenty-six are gone, and the final frontend candidate
  measures 63 warnings against a ceiling of 63. Nearly all described one habit — state written in an effect to
  correct what the render before it already showed — so those values are
  resolved where they are used, and the ones describing a particular thing now
  carry which thing. Two genuine exceptions are declared at the line with their
  reason: an image array whose identity changes every render, and the registry
  of in-flight AbortControllers, which the rule would have held in state.

- An action card aimed at another project's Quick Prompt launches, and says
  where it will run. The preflight refused it outright, which made a proposal
  unusable for the one thing it was for. The whole mechanism is human-gated —
  the agent proposes, Kronn checks the target, a person decides by clicking —
  and the checks that matter all still run: the target exists, its variables
  match its contract, the proposed project is the target's own, and that
  project exists. What the guard was refusing after all of those is a decision,
  not a risk. The card now carries the target's project, quietly and always,
  because lifting the guard without saying where the action goes would be worse
  than the guard. Discussions and Live Pages changed together: the same gesture
  cannot mean two things depending on where the card is drawn. Server-side
  resolution of a Live Page's dataset-bound values is untouched.

- Notes have a place of their own. An agent could already ask for a clean list
  of a discussion's notes; the route that served it was never called from this
  side, so the person who wrote them could not read them back — theirs were
  mixed into the transcript, behind a switch that showed all or none, with no
  way to list, correct or remove one. There is a Notes panel now, beside the
  others: it lists them, writes them where the list already is, corrects one in
  place and deletes it through the tombstone Kronn already had. A room with
  months of notes loads them a page at a time rather than in one request.
- Search can be pointed at notes alone. The scope filter gains a fourth
  option beside title, content and both: "notes only", for when the note is
  what you are after and the transcript around it is noise. Scoping to notes
  drops title matches with it — a note has no title of its own, and keeping
  them would quietly widen the search back to the whole discussion.

- An edited note says so, with the date. Silently replacing what someone wrote
  for themselves is the one thing an editable note must not do, so the panel
  carries the moment of the last rewrite — and stays quiet on a note that was
  never touched. On a note somebody else wrote, the edit and delete controls
  are hidden and their author is named instead. That is a guard rail in the
  interface, not an authorisation: Kronn has no multi-user authentication, the
  pseudo is declarative, and the endpoints behind those buttons accept any
  caller. It keeps a shared room from being tidied by accident; it prevents
  nothing. An unsigned note counts as this machine's own, so nobody is locked
  out of the notes they wrote before setting a pseudo.
  Successive corrections now share the discussion's ordered revision sequence.
  Updating the text and recording its history are atomic: an audit failure
  leaves the original note intact instead of changing it behind an error.
  Shared-note corrections keep their note semantics on the receiving peer:
  they no longer remove later conversation replies, change the note's written
  date, or replace its routing. Synchronization and the revision trail remain.

- A deleted note leaves the notes list. Its tombstone stays in the transcript,
  where a gap has to be explained, but a list of notes is not a transcript:
  showing the marker there would be listing something its author removed. The
  agent tool reads the same list and stops seeing it too, and the count agrees
  with what the list returns.

- A task can be taken out of a discussion's plan. It could be added, and then
  only moved between "active" and "later" — never removed. A room that runs for
  weeks accumulates work belonging to the next release, and reading its plan
  meant reading past all of it. `task_unlink_discussion` is the reverse of the
  tool that adds one; the task, its history and its other discussions are kept.
  It fits under the MCP surface ceiling rather than over it: the four heaviest
  descriptions it pushed past the line were tightened by nearly what it costs,
  and the ceiling moved by six bytes to admit it, then came back down as the
  surface was trimmed further.

- Kronn stopped telling agents to run linters the project does not have. A
  `composer.json` was enough to write `Lint: phpcs` into all eight generated
  instruction files, but phpcs is an optional Composer package, absent from
  most PHP projects — so every agent read a command that could not run, and
  the contradiction propagated eight files at a time. It is now declared only
  when something proves it: the vendored binary, a `require-dev` entry, or a
  ruleset. Ruff had the same flaw, since it does not ship with Python either,
  and a `lint` script was chained onto `tsc --noEmit` whether or not
  `package.json` defined one — which made the whole command fail where it did
  not. Linters that come with their toolchain, clippy and `go vet`, are still
  assumed from the language.

- Codex and OpenCode list their models again. Both had moved to shapes the
  catalogue reader did not know — Codex now describes each reasoning effort as
  an object instead of a bare name, and OpenCode spells its session options
  `currentValue` and `options[].value`. Neither is standardized, and a
  catalogue left in error refuses every turn for that agent, so both readers
  now accept the old and the new spelling. Codex also supplies a display name
  of its own ("GPT-5.6-Sol"), which beats showing the bare id.
- A message with no one mentioned now says where it goes, and reaches it. In a
  room whose native agent is switched off, an ordinary turn used to resolve to
  no destination at all — the native responder disabled, no peer named — so it
  reached nobody and the writer was told nothing. It now goes to the sessions
  joined to that room, which is what switching the agent off already promised.
  The header names that destination, resolved by the router itself rather than
  guessed from the discussion's configured agent, because that setting survives
  being switched off and announced a reader who received nothing. When there is
  genuinely no one, it says so.
- An agent reached over ACP — OpenCode and the other native ACP runtimes — now
  receives Kronn's own MCP bridge, so it can answer in the room it was invited
  to instead of joining it mute. Only the command travels over the protocol:
  the bridge reads its credentials from the environment it inherits from the
  process Kronn spawned, so none is ever serialized into an ACP payload.
- A generation asking for a source image on a provider that takes none is
  refused at submission, naming what is missing, instead of queueing a job that
  retries the same refusal until its deadline expires.
- A provider failure no longer ends a turn in complete silence. When the API
  is overloaded or a spend limit is hit, Claude Code does not report a
  structured failure — it writes an assistant message of its own, which no
  streaming delta precedes. The stream reader skipped every assistant message,
  rightly so for ordinary ones (they repeat what was already streamed, and
  keeping them would print each reply twice), and in doing so discarded the
  only account of the failure. The turn ended with nothing at all, which is why
  a room could sit empty for 18 minutes, once for an hour and a half, before
  someone retried by hand. Errors the CLI writes itself are now read — and only
  those, identified by two independent markers so a build that stops setting
  one still surfaces them. Found in real transcripts rather than deduced: the
  database held five persisted API errors and not a single 529.

- An action whose target was deleted can no longer be launched. Preflight runs
  when the proposal is ingested, but the click can come much later — a
  transcript, and a published page, outlive the Quick Exec they name. Nothing
  checked again at launch, so the click was accepted for something that no
  longer existed. The action now says the target is gone instead. The check
  lives in the launch engine both surfaces share, so the guarantee cannot hold
  in a discussion and lapse on a page.

- A prompt's version history no longer outlives the prompt it belongs to.
- A clip recorded as `text/plain` still plays as a clip.
- A Quick Prompt batch run stops accepting a per-item prompt that nothing ever read.
- The agent bootstrap (`docs/AGENTS.md`) is back under its context ceiling
  without the ceiling moving. The 716 bytes over were exactly what 0.13.0 had
  added: two verbose rows in the task router and a section holding a single
  pointer. The router now routes and stops explaining — every detail it
  dropped already lives at the destination it points to — and the sections
  that only carried a pointer became rows of that same table. The inventory
  of root redirector files moved to `docs/repo-map.md`, where repository
  structure is documented.

- Two CLI sessions of the same provider joined to one room — two Claude Codes,
  say — now both appear among the participants, and each stays mentionable by
  its own alias. A session that was working rather than listening read as
  Offline, and the rule that hides the stale row left behind by a reconnecting
  CLI could not tell that row from a live peer: it hid whichever session was
  busy, so one Claude could not see or address the other. A row is now hidden
  only when it is Offline and nobody has heard from it for a while; a working
  session touches `last_seen` on every append and every wait, so it stays.

- Sending a message in a room with joined CLI agents, in a human-only
  discussion, re-sending a duplicate, or revising a message no longer raises
  "SSE stream closed before a terminal event" on every turn. The frontend had
  learned on 1 September to read a stream that closes without a terminal event
  as an interruption — which is what protects a half-written reply across a
  backend restart — while five backend paths still ended their stream on
  purpose without saying so. Those paths now close with an explicit `complete`,
  so the interruption rule stays true and stops firing on finished turns.

- Listing a workflow's runs no longer carries every step's full output. A run's
  `step_results_json` averages 470 KB, and `output` is all of it — measured at
  100% of a 3.9 MB row, every other field together under 700 bytes — yet that
  column travelled through each listing, decoded and re-encoded, to render rows
  showing only a step's name and status. One workflow's runs answered with
  237 MB in 8.7 s, and the run detail's fan-out progress asked for the same page
  every 8 seconds. Outputs are now blanked inside SQLite on the listing paths:
  9.3 MB → 28 KB for a page of ten, 241 MB → 1.3 MB for the 500-run cap, and
  14.6 MB → 33 KB for `/api/workflows`, which had been decoding each workflow's
  last run in full only to keep five fields of it. Opening a run still serves
  everything.

- A discussion turn that fails on a saturated provider is retried instead of
  stopping silently. Two `529 Overloaded` turns simply ended with nothing shown
  and nothing relaunched; the user waited 18 minutes once and 1 h 27 the other
  time before restarting by hand. Saturation now retries with a growing delay,
  and the last attempt keeps the provider's own message in the room rather than
  going quiet. A hard quota is never retried — it is checked first — and `529`
  is never matched on its own, since those digits appear in ids, token counts
  and durations where a false positive would spend a real API call.

- The durable error of a failed turn no longer describes an ordinary room as a
  task worker. Every unsuccessful turn wrote the same "task-worker
  startup/completion failed … use `task_exec_reassign`" text, which sent readers
  looking for a sub-discussion that never existed and overwrote the provider's
  actual reason. Real task workers keep that guidance; an ordinary room now
  states the cause it actually hit.

- An agent that cannot start because of a NUL byte in its command line now says
  what carries it — an environment variable by name, an argument by the flag it
  follows, the program name, or the working directory — instead of repeating
  `nul byte found in provided data` every 30 seconds. One report shows the same
  refusal replayed 282 times without diagnosing anything: the failure is settled
  before the process runs, so it is now a hard preflight failure, surfaced in
  the discussion. Values are never logged, only their carrier.

- The stream now says when a tool STARTS, not only when it finishes. A single
  Bash call can run 80 seconds, and for that whole time the interface sat on a
  frozen placeholder while Kronn already knew which tool was running.

- A Page's inline Kronn action CTAs (`data-kronn-action`) now work from the
  standalone tab and every mosaic tile, not only the embedded viewer: clicking
  one opened only a same-origin link relay with no action handler, so the
  click was silently intercepted and lost. The three surfaces now share one
  `useLivePageActions` hook and the native `LivePageActionCard`, loading and
  validating each Page's own action list, failing closed on a removed
  `action_ref`, and keeping every mosaic tile's action state fully isolated
  from its siblings. A terminal action's "open discussion" jump opens a fresh
  same-origin tab since these surfaces have no Dashboard shell to navigate
  within.

- OpenCode no longer gets locked out of a discussion just because
  `~/.local/share/opencode/auth.json` is missing, empty, or unreadable.
  OpenCode also accepts environment credentials, its own provider config, and
  local or no-auth providers Kronn cannot see from that one file, so the
  absence of a confirmed auth signal now reports "unknown, assume runnable"
  instead of a hard "not ready" that blocked dispatch outright.

- The image/video model pickers for an HTTP connection (Settings > Agents >
  External API) no longer go silently empty for a provider whose catalog
  carries no modality metadata, as with NVIDIA's `/v1/models`: every one of
  its entries used to be treated as chat-only, hiding its real image/video
  models behind a filter that could never match. The picker now reads
  catalog evidence and renders one of three states — known (strict filter,
  unchanged for OpenRouter), unknown (the full catalog behind an explicit
  warning, search and free-text entry preserved), or unsupported (the
  connection proves it has no compatible model). A saved selection is never
  cleared by a refresh or a re-test, whatever the state.

- A task execution parked on a CLI worker's control offer — waiting for the
  exact target session to accept — can now be redirected to another worker.
  It used to refuse outright with "not a resumable worker state", stranding
  real work behind a CLI that never showed up. The same execution, checkout,
  history, attempts and review budget carry over untouched; the stale offer
  is cancelled in the same move, so the original CLI can no longer accept it
  late. Every other blocked reason — a session already committed elsewhere,
  or a protected-merge checkpoint — still refuses reassignment and requires
  a human decision, unchanged.

- An ACP agent that fails to start no longer leaves its process running. Nine
  early returns ended the session without stopping the host it had just
  spawned; `shutdown` killed without reaping, so the entry stayed in the
  process table; and the task draining the agent's stdout owned nothing — a
  dropped join handle detaches instead of cancelling, which left the drain
  alive after its process was gone. A start failure now terminates the host on
  every path while preserving the error that caused it, the process is killed
  and reaped, and the drain is joined under a bound: a panic still surfaces as
  a panic, and only the cancellation Kronn asked for is treated as ordinary.
  Proven against a real subprocess observed over a socket, with no PID polling
  and no sleeps.

- An identity Kronn does not recognize is refused instead of quietly becoming
  a Custom agent. Two database readers turned any unknown `agent_type` into
  `Custom`, and an unknown connection preset took the same road through
  `Other`, so corrupt data was routed as a working agent instead of being
  reported — on every read path that resolves message targets, session rows,
  the peer import and the model catalogue. Those readers now fail closed, the
  way the orchestration reader already did. Explicitly stored `Custom` and
  `Other` keep their established routing, and the error deliberately omits the
  offending value rather than echoing corrupt data back. The rule is written
  down in `docs/decisions.md`, where the next reader will look for it.

- OpenCode gets a runnable Linux copy on a macOS host, like the agents it sits
  next to. Its Darwin binary cannot exec inside the Linux container, so Kronn
  skips it — but nothing installed a Linux one in its place, and the agent
  quietly fell back to the npx runtime, which re-pays a Node cold start on
  every session. It is now mirrored at container start like Claude, Codex,
  Gemini and Copilot: only when the user actually has it on their Mac, never
  reinstalled when a container copy already exists, and a missing or failing
  npm degrades with a diagnostic instead of stopping the container. The boot
  mirror also gained its first tests — the mechanism had produced two silent
  detection bugs before this one, and a parity check now names any agent left
  skipped without a replacement.

## [0.12.0] - 2026-08-30

### Added

- Settings now manages any number of named OpenAI-compatible connections in
  one External API zone. LiteLLM, NVIDIA and OpenRouter have dedicated presets,
  while `Other` accepts another compatible endpoint; each connection has its
  own discussion mention alias, credential and Economy / Default / Reasoning
  model mapping. Existing LiteLLM and NVIDIA settings migrate into named
  connections.

- Project details include a Docker tab with Compose service status, bounded
  project and service lifecycle controls, published ports and hosts, host-file
  diagnostics, one-click URLs and recent logs. Project cards expose a running
  badge and a matching collection filter.

- Live Pages selected together can open in a standalone mosaic. Two- and
  three-Page layouts offer explicit arrangements, while larger selections use
  a responsive grid without merging the Pages' sandboxed runtimes.

- Full audits now finish with a deterministic documentation-optimization gate
  that measures the context loaded by each detected agent integration and blocks
  validation when mandatory documentation is oversized, broken or ambiguous.

- Delegated task details expose durable native progress phases, reliable signal
  timestamps and honest telemetry availability instead of inferring activity
  from an attached browser stream.

- Ollama model downloads expose streamed pull progress in Settings and can be
  cancelled while the exact pull is still running, with explicit success and
  failure outcomes instead of one opaque request.

### Changed

- Projects, Discussions, Planning, Automation, Pages and Plugins now share the
  same collection sidebar structure: compact header actions, search, separate
  filter and sort controls, Favorites / Recent / All groupings, row actions,
  keyboard navigation, responsive collapse behavior and shortcut footer.

- Project details separate Audit, Docs and Code into direct full-height tabs.
  Audit launch uses the shared agent selector and sixteen-step full-audit
  briefing, documentation health is explained in plain language, and telemetry
  coverage moved from project cards to Settings.

- External API connection cards use one compact visual hierarchy for endpoint,
  credential state and tier mappings, with inline create, edit, test and delete
  actions. Model mappings stay locked until the current endpoint and credential
  pass a connection test.

- Agent and project pickers use the same searchable, keyboard-accessible
  selector across discussions, Quick Prompts, Quick APIs, workflows and audit
  launch. Quick Prompt comparison has one unambiguous launch action, labels
  named external providers and lets users copy run identifiers.

- Compatible frontend, backend and desktop dependencies were refreshed for the
  release. The unused `backoff` dependency and its unmaintained `instant`
  transitive dependency were removed; the remaining allowed Rust maintenance
  advisory is inherited from the PDF extraction stack. The release-time CLI
  freshness registry was also refreshed against each vendor's stable channel.

- The repository-wide duplicated-line ceiling is ratcheted from 4% to 3%
  against a measured 2.62% candidate baseline, preventing gradual copy-paste
  growth as the codebase expands.

- Backend coverage floors are ratcheted to 83% for lines, functions and
  regions, with the security-sensitive key-management module floors tightened
  to their demonstrated 90–99% range.

### Fixed

- Delegated-task worktrees release shared edit locks deterministically, retain
  commit authority through bounded recovery and report native provider progress
  without presenting missing telemetry as a stalled or free session.

- The long-lived `kronn-internal` MCP bridge can reload when its loaded source
  changes through a versioned, owner-only and size/schema-bounded handoff,
  without trusting a mutable replacement artifact between verification and
  execution.

- External connection tests invalidate stale model selections, bound concurrent
  probes and verify an entered credential with an authenticated minimal request
  instead of trusting a public model catalogue. Migrated NVIDIA connections
  retain their executable default endpoint. OpenRouter uses a non-billable key
  validation endpoint, preserves the full key prefix and upgrades already
  receipted databases without violating foreign keys.

- Re-running a Quick Prompt comparison preserves each target's provider and
  reasoning tier instead of collapsing every result onto one agent, while
  resolving the model currently assigned to that tier.

- Audit/template installation now creates the shared `AGENTS.md` entry point
  unconditionally but emits Claude, Gemini, Cursor, Windsurf, Cline, Copilot,
  Kiro and Vibe instruction adapters only when the target repository already
  declares that integration or the user explicitly launches it for bootstrap
  or audit. Generated adapters are rendered without raw placeholders or
  example commands, and a bounded upgrade repairs only recognizable
  Kronn-managed template ranges while preserving user content.
  Localized briefing prompts now consistently reference the canonical `docs/`
  tree instead of the retired `ai/` path.

## [0.11.0] - 2026-08-22

### Added

- Planning tasks can now be delegated through a durable orchestration lifecycle:
  one selected worker receives a child discussion and isolated managed worktree,
  while the parent discussion follows progress, reviews typed delivery evidence,
  requests bounded changes, reassigns on provider failure and integrates only
  through validated fast-forward with target-SHA and backup-ref guards. See the
  [operator guide](docs/guides/task-orchestration.md).

- Quick Prompt comparisons now include reorderable result columns, model,
  duration and token metrics, independent human and blind AI quality ratings,
  rankings over weighted/AI/human quality, time or tokens, and a reasoning-agent
  path that opens a contextual discussion to improve the source prompt.

- Ollama, LiteLLM and NVIDIA HTTP agents can use Kronn's native tool catalogue,
  preserve honest partial evidence when a tool/context limit is reached and act
  as lower-cost or local fallbacks when hosted CLI providers are unavailable.

- Kronn can now tell you which of a plugin's endpoints keep failing. A plugin
  whose endpoints fail repeatedly carries a "spec to check" badge on its card,
  naming the endpoint and the status behind it. It reads the call log Kronn
  already keeps (`GET /api/api-call-logs/drift`), and separates an endpoint that
  has never answered — a spec that was wrong from the start — from one that also
  succeeds, where the endpoint exists and the call is malformed. The two need
  different fixes, so they read differently. The badge stays out of the way until
  failures accumulate: an alarm raised on healthy plugins is one nobody reads.

### Changed

- The Custom API builder must verify an endpoint before declaring it. It used to
  read the documentation and write down what that implied; documentation goes
  stale, and the resulting spec sent agents chasing endpoints that answer 404 or
  parameters the API ignores. It now tests each endpoint with a real call when it
  can, marks anything unverified as such instead of asserting it, and records the
  response shape and the parameter names that actually work.

### Fixed

- The orchestration bridge no longer makes every principal session pay for the
  full Ollama worker methodology and duplicated scope schemas in its MCP
  catalogue. Selection-time contracts stay visible; exact transport, bounded
  scope, validation and fallback examples now load through `tool_manual` only
  when a principal delegates. This restores the ratcheted catalogue budget while
  keeping spawned local workers on their dedicated two-tool surface.

- The `kronn-internal` bridge now fingerprints the script it actually loaded and
  refuses orchestration mutations whose optional security fields could otherwise
  disappear through a stale MCP schema. After upgrading to 0.11.0, reconnect
  every already-running `kronn-internal` MCP process once and verify
  `bridge_info` reports `stale: false`; a pre-0.11.0 bridge cannot protect the
  transition that made this guard available. Recovery and completion reads stay
  available while a bridge is stale, so an existing execution can be inspected,
  cancelled, reviewed or delivered without replaying its launch.

- Spawned host task workers no longer need write access to a linked worktree's
  shared Git objects or refs. Codex and Claude remain confined to the managed
  worktree and commit through an execution-bound Kronn tool before delivery.
  Git commit endpoints now also treat their explicit file list as authoritative:
  unrelated paths already staged in the index are left staged and cannot be
  included accidentally.

- Native `kronn start` no longer leaves the UI and API offline while another
  Cargo command owns the repository's shared build lock. The supervisor serves
  the last successfully-built backend during the wait and only swaps it when
  the completed build actually produced a different binary.

- API calls from General discussions now execute configurations explicitly
  enabled for General instead of advertising them through `mcp_list` and then
  rejecting them for having no project. The shared API executor also rejects
  unknown or missing path parameters before the network call and names the
  exact parameters expected, replacing opaque vendor 404s with an actionable
  broker error.

- Plugin configurations that are global to nothing, available to no general
  discussion, and linked to no project no longer look healthy while remaining
  invisible to every agent. The Plugins page flags the orphan scope, offers a
  one-click repair through General discussions, and prevents the UI from
  removing a configuration's last remaining scope.

- Live Pages workflow Sync now distinguishes its three outcomes at the action
  itself: a spinner while running, a green check on success, and a red cross
  with the returned error message on failure. A successful run no longer looks
  like a failed validation.

- The discussion plan's compact “+N more in progress” and “+N more ready”
  indicators are now controls: they reveal the remaining tasks in rank order
  and can collapse the list again. Their state resets when switching
  discussions.

- A tool result that had to be trimmed no longer lets an agent report a cut list
  as a whole one. Trimming a document loses text and it shows; trimming a
  collection changes its meaning — one entry out of forty-three reads as "there
  is one", which an agent then states as fact. Kronn now keeps every entry as a
  compact identifier record when that fits; otherwise it emits valid JSON for
  the retained prefix, says exactly how many items it kept out of the total, and
  warns that the count is not final. The Fastly service endpoint also tells
  agents to project `id`, `name`, and `version` instead of loading every
  service's full version history.

- A discussion whose agent is actually working now shows as working, even when
  this browser never opened its stream — a run started from the API, from
  another tab, or before a reload used to sit under the queued hourglass
  alongside the ones genuinely waiting. The sidebar reads the dispatch's real
  status instead of relying on a live-stream flag it may never have received.

- The SpeedCurve plugin describes its API as it actually behaves. Its spec named
  `site` and `since`/`until` where the API wants `site_id` and
  `start_timestamp`/`end_timestamp` — and an unknown filter is accepted and
  silently ignored, so agents analysed another site's data believing they had
  filtered. Four LUX endpoints that answer 404 were removed, pagination is
  documented, and the guidance no longer points at them.

- A model whose tool budget is spent is now made to answer rather than asked to.
  Refusing the call left the declarations in place, so it simply asked again —
  eleven more times, until the round cap. The declarations are withdrawn once a
  whole turn has been refused, which is what the "you used tools but wrote
  nothing" retry already did.
- The workflow detail pane no longer shows a second scrollbar inside the page's
  own. The panes declared `overflow-y: auto` with `overscroll-behavior: contain`
  while having nothing to scroll, which swallowed the wheel; the surrounding
  viewer is the single scroller again.

- A model circling the same tool is stopped after twelve calls to it, instead of
  running until the round cap. The existing guard only caught a call repeated
  argument for argument, so varying one parameter each time slipped past it —
  observed as 47 `api_call`s over 47 minutes, ending with nothing. Kronn now
  refuses the thirteenth and tells the model to answer from what it already has.
- Tool traces name the route an API call took — which plugin, which endpoint,
  which method — instead of a bare `api_call() → ok`. Query strings and bodies
  are still never shown: the route identifies a call, the values are the part
  worth protecting.

- A short question to a local agent no longer dies on its first tool call. The
  context window was sized from the prompt alone, so a one-line question got a
  near-floor window that the first tool result blew past — and since Ollama fixes
  that window when it loads the model, raising it afterwards bought nothing. A
  turn that declares tools now asks for the full ceiling up front, and oversized
  results are trimmed against the window actually granted rather than the one
  that was theoretically available.

- An agent working through tool calls no longer looks dead. A batch progress
  tick was clearing the per-discussion "running" state, which unmounted the
  streaming bubble mid-run — taking the live tool traces and the elapsed counter
  with it — and flipped the sidebar card back to the queued hourglass while the
  job was still running. A progress tick means the batch advanced, not that this
  discussion finished.
- The workflow detail pane scrolls with the wheel again. Its container had an
  automatic height, so the pane grew with its content instead of overflowing:
  nothing scrolled inside it and reaching the bottom meant dragging the page
  scrollbar.

### Changed

- An agent exploring a repository is no longer cut off after eight tool rounds.
  That ceiling was written for an agent calling one API — "list, then call, then
  maybe retry once" — and never revisited when HTTP agents gained file and git
  tools: finding files costs two or three rounds before the first read, so a
  triage that was genuinely working died with its report unwritten. The
  configured execution duration is now the real limit; the round cap remains as
  the anti-runaway backstop underneath it.
- Tool traces name what they did: `find_files({"pattern": "..."})` rather than
  `find_files()`. Only the workspace tools, whose arguments are paths and globs
  in your own repo — `api_call` and friends stay name-only, since their
  arguments can carry secrets. Eight identical-looking `find_files() → ok` lines
  said nothing about what was actually searched.

- A batch item now gets the execution ceiling the operator configured, instead
  of a fixed twenty minutes. A batch item is one agent run, so "maximum agent
  execution duration" applies to it too — a local model legitimately spending
  many tool rounds was being cut off well before that setting. It remains a
  guard, not a switch: the value stays clamped to 1–120 minutes.
- A run stopped by that ceiling no longer claims the user interrupted it. The
  cancel signal carries no reason, so the message states what happened and names
  both causes rather than asserting one it cannot know.

### Added

- Each agent now carries its own concurrency limit, set on its card in Settings.
  A local agent is capped because the machine is the limit — Ollama defaults to
  1, since it serves a single inference slot and a second run only queues and
  discards the KV cache the first one warmed; a CLI agent defaults to 5. Remote
  providers stay unlimited: LiteLLM and NVIDIA are endpoints someone else
  scales, and a cap there is about spend, not this machine. Admission is atomic,
  so a job whose agent is at its limit stays queued and no request is sent.

### Fixed

- An Ollama agent that read a large file or diff no longer fails with a bare
  "API server error". The context window was sized once from the system context
  and the user prompt, then never re-sized as the tool loop appended results, so
  Ollama truncated the history until the user turn itself was gone and rejected
  the request. The window is now re-sized from the messages actually being sent,
  and the truncation failure reports what happened instead of pointing at tool
  calling. A single result too big for the window at its widest is trimmed rather
  than dropped, and says how many bytes went missing so the model does not reason
  on a silent truncation.

- Local Ollama steps no longer pay for reasoning tokens they discard. Kronn sent
  qwen3 models the `/no_think` control token, which recent Ollama runtimes have
  made nearly inert; the request now also carries `think:false`, which they do
  honor. On a classification prompt the generated-token count drops from 240 to
  2 (`qwen3:8b`), 226 to 1 (`qwen3.6`) and 82 to 1 (`qwen3.8`) for the same
  answer. The flag is only ever sent to turn reasoning off, so a model Kronn
  makes no claim about keeps its own default.
- The agent freshness pill works again. Every latest-known-version constant had
  fallen behind — Ollama was pinned to 0.4.7 against a current 0.32.14 — so no
  agent was ever reported as out of date.

## [0.10.0] - 2026-08-14

Kronn can now turn scheduled API collection into a readable, continuously
updated HTML report without sending mechanical data work through an agent.

### Added

- **Live Pages** provide versioned HTML reports backed by named JSON datasets.
  Pages render in a credential-free sandboxed iframe, retain data and HTML
  history independently, and can be linked to workflows or discussions. MCP
  agents may also create standalone Pages without first joining a Discussion;
  the current or an explicit Discussion remains an optional provenance link.
- The Pages library reuses the Discussion navigation model with search,
  favorites, multi-selection, archive and explicit deletion. Its navigation
  entry appears only after the capability has been activated. Each Page also
  exposes a compact header dropdown for its three latest successful workflow
  refreshes, with publication time, data revision and dataset-level `modified`
  / `unchanged` deltas, per-dataset retained JSON size, and direct navigation
  to the exact originating run. A linked workflow can also be relaunched from
  this dropdown without leaving the Page. The HTML studio adds line numbers, syntax
  highlighting, revision history, side-by-side comparison and restore-to-draft.
- Three deterministic workflow steps cover the complete data path:
  `CollectApiData` runs saved Quick APIs and saved shell-free, allowlisted CLI
  collectors concurrently under stable aliases, `TransformData` selects and aggregates typed JSON with bounded
  JSONPath recipes, and `PublishPageData` atomically updates one or more Page
  datasets. All three consume zero model tokens.
- **Quick Execs** are now reusable Automation resources with create/edit/test,
  project binding, declared variables, safe literal argv, bounded execution and
  JSON/CSV/text/lines normalization. Agents receive symmetric `qe_list`,
  `qe_create_draft`, `qe_update` and `qe_run` MCP tools, and Workflow Architect
  composes them through `CollectApiData.sources[].quick_exec_id`.
- Workflow authors can test a complete collection, reuse the previous step's
  real sample, map fields from an interactive JSON tree, preview the transformed
  result, and add a Transform or Page step directly from the collector.
- Page headers export a browser-rendered capture of the materialized report
  (runtime dataset content, authored CSS, SVG and canvas charts) to PDF or
  fixed-layout DOCX, preserving CSS that the document sidecar cannot interpret. Dataset totals
  open an inline viewer with a dataset selector, tabular preview, retained size,
  and CSV/JSON export; the refresh dropdown remains a second entry point.
- Workflow export bundle v3 now carries and remaps referenced Quick Prompts,
  Quick APIs, Quick Execs, Page templates/contracts and transitive sub-workflows. Credentials
  and retained Page observations remain local by design.
- Quick APIs and workflow API steps support vendor-neutral, run-anchored time
  expressions with signed offsets, IANA timezones, minute/hour/day flooring and
  RFC 3339, local ISO, date or Unix formats. Parallel collectors and resumed
  runs reuse one durable timestamp, so rolling cron windows cannot split across
  an hour boundary.
- The Kronn MCP and Workflow Architect expose Page discovery/authoring and the
  full data-pipeline schema, allowing an agent to create standalone or
  mock-backed Pages and then wire them to real workflow data.
- **Continual Learning** ships as an explicit, default-off beta. Once enabled,
  agents may propose durable facts, preferences or pitfalls through a typed MCP
  tool, but evidence checks and a human decision are required before Kronn
  writes to the dedicated user or project learning document. Pending candidates
  remain visible from a global badge and stale evidence is rechecked on a
  bounded background sweep.
- Every Discussion header now opens a searchable **Assets** inventory for all
  shared or agent-generated files in that room. Images keep the in-app
  carousel, filters separate images/files/pending uploads, large histories load
  forty cards at a time, disk-backed files can be downloaded, and each attached
  asset links back to its source message. The Assets and modified-files header
  actions stay hidden while their respective counts are zero.

### Changed

- Quick Prompt Compare now selects explicit agent + model-tier targets with
  the same picker used across Kronn, including comparisons of two tiers of the
  same agent. A launch opens a dedicated in-app comparison workspace with rich
  Markdown columns, live run states and links to each durable child discussion;
  any existing batch can reopen the workspace from its actions menu.
- The public-site screenshot gallery now opens in an accessible in-page
  carousel with keyboard navigation while preserving modified-click new-tab
  behavior. Its reproducible demo seed includes public-safe repository content
  plus an Automation showcase and isolates MCP sync from the real user home.
- The Automation library now follows the same sidebar + detail interaction as
  Discussions and Pages. Workflows, Quick APIs, Quick Prompts and Quick Execs
  share one ordered, searchable, project-filterable navigation surface instead
  of four unrelated tab layouts. Selecting a Quick API, Prompt or Exec opens its
  editor immediately in the detail pane; launch, compare, batch, history, ID,
  export and delete controls remain available in a compact command header. Long
  Quick Exec invocations stay on one ellipsized, inspectable line so AWS queries
  and structured arguments cannot make that header consume the viewport. The
  library labels them as `Quick Execs (CLI)` and their Test action now uses the
  same primary CTA treatment as Quick Prompt launches. The guided tour now
  introduces the unified library, follows the sidebar order and includes Quick
  Execs instead of describing the former tab layout.
- The primary navigation now follows the product flow: Projects, Discussions,
  Planning, Automation, Pages, Plugins, then Config. Pages therefore sits next
  to the workflows that publish it, while infrastructure remains grouped last.
- `CollectApiData` now surfaces a failed Quick Exec's stderr in its summary,
  gives expired AWS SSO sessions an actionable `aws sso login --profile …`
  diagnostic, and fails an entirely empty collection even when every source is
  optional. Optional failures remain `PARTIAL` only when another source
  actually produced data.
- Page destinations are shared resources rather than workflow-owned children:
  several workflows can publish different datasets to the same Page, while
  both Workflow and Discussion links remain visible from the Page library.
- Workflow previews now label data steps with their human-readable type and
  useful source/field counts. A `PublishPageData` preview links directly to its
  target Page.
- Disabling Continual Learning stops capture and removes its injected project
  pointer without deleting previously validated learnings; existing pending
  candidates can still be reviewed so the queue can be drained safely.

### Fixed

- Discussion runs now distinguish the configurable inactivity watchdog from a
  separately configurable absolute execution duration (1–120 minutes). The
  former hidden 30-minute constant no longer terminates healthy, actively
  streaming agents after operators explicitly allow longer runs.
- Joined CLI agents can publish local images and files with `disc_append`.
  Kronn uploads them through the authenticated context-file path, pins only
  those files to the exact message, and refreshes the open discussion live.
  Clicking a thumbnail opens an in-app gallery with previous/next navigation,
  keyboard controls, an image counter, and an explicit new-tab action.
- Historical message attachments no longer consume the 20-file composer
  staging limit, returning to an already-open Discussion refreshes a stale
  attachment cache, and a failed multi-file MCP append compensates every upload
  from that batch instead of leaving hidden pending files behind.
- Time-series retention preserves append order when several points share the
  same observation timestamp instead of using random UUID order as a tie-breaker.
- Quick API POST bodies are sent as their typed JSON value instead of being
  serialized twice. Manual Quick API calls and workflow `ApiCall` steps now use
  the same body contract, including audit-log attribution.
- Global Quick APIs can be tested from a collector even when the workflow has
  no project, while project-scoped APIs still fail closed without their project.
- Transform previews now preserve the selected JSONPath result instead of
  presenting a misleading nested projection.

## [0.9.7] - 2026-08-13

This release is a reliability sweep across discussions, workflows, project
audits, plugins and desktop packaging. GitHub Copilot CLI issue #150 remains in
the backlog because that provider is no longer available in the test
environment; it is not presented as fixed.

### Added

- LiteLLM and Ollama Agent workflow steps can use a bounded set of Kronn-native
  tools for configured APIs, Quick APIs and read-only Planning. Tool execution
  stays project-scoped, secrets remain server-side and run details retain only
  the tool name and outcome—not arguments or credentials. A completed HTTP
  Agent step with no recorded call says so explicitly and points operators to
  a tool-capable model or deterministic `ApiCall` step when external data was
  expected.
- Plugin imports now end with an explicit assignment table. Every imported
  configuration starts with **Global** selected for convenience, but Kronn
  applies that scope only after confirmation; operators can instead select one
  or more projects.
- Context Audit snapshots make documentation drift visible on project cards,
  and existing documentation can be explicitly attested without pretending an
  AI audit ran.
- Desktop CI now smoke-tests PDF and DOCX generation before Tauri packaging,
  preserves platform diagnostics and requires non-empty Windows, macOS Intel,
  macOS ARM and Linux installers before a release can proceed.

### Changed

- All discussion agents now receive the same compact rich-output contract.
  Native and CLI agents can intentionally produce Mermaid diagrams, sandboxed
  HTML previews with PDF/DOCX actions, or CSV/XLSX/PPTX export cards without
  relying on the document-generation skill having been auto-selected. Mermaid
  diagrams now expose shared 50–250% zoom controls in inline and fullscreen
  views, with scrollable overflow for dense graphs.
- HTTP discussion agents are told that configured REST APIs are Kronn-native
  tools, not MCP servers, and are directed through API/Quick API discovery
  before inventing an unavailable vendor integration.
- Plugin cards expose registry/configuration drift and retired registry entries
  instead of silently rewriting encrypted configuration. Microsoft Graph's
  Docker CLI-token path again uses the authenticated Azure CLI when available.
- Workflow documentation now describes the exact, limited overlap with OpenAI
  Symphony: four workspace-hook names are shared, but Kronn does not import
  `WORKFLOW.md` and does not implement Liquid templates.
- Serious/critical axe findings on the five main pages now have a zero baseline;
  future browser failures attach the precise violation targets as diagnostics.

### Fixed

- Native discussion-agent startup failures are no longer left as silent,
  indefinitely pending targets. Catalogue/model errors and temporary provider
  outages now share one compact diagnostic card; the latter can retry only the
  failed agent against the original user turn without replaying successful
  sibling agents. Legacy structured LiteLLM 404 messages remain readable.
- Resuming a CLI session can no longer rotate credentials into a different
  discussion. Peer traffic receipts are cursor-based and content-free, so an
  agent can detect unseen work without rereading the transcript.
- Shared and inherited workflow worktrees retain durable ownership while child
  jobs need them. Restart reconciliation cancels stale workflow children,
  refuses unsafe fire-and-forget fan-out and purges only managed terminal
  worktrees.
- Unknown workflow template variables, malformed filters and unclosed
  placeholders fail before a step performs an API call, command, notification,
  gate or agent invocation.
- Project audit badges distinguish installed templates, bootstrap evidence,
  completed audits, human attestations and validation without allowing legacy
  backfill markers to overwrite a newer state.

## [0.9.6] - 2026-08-12

Kronn's own token cost, measured and then bounded. Every figure below is a byte
count from this machine; token counts are estimates and gate nothing.

Full release documentation, including what each measurement does NOT prove and the
rollback matrix: `docs/operations/token-economy-0.9.6.md`.

### Added

- Workflow step details now open as a compact inspector with a familiar
  Preview/Edit switch. Preview remains the default; Edit embeds the canonical
  step editor in place, keeps long forms scrollable, saves without walking the
  whole wizard, and preserves Cancel as a no-write action. A selected late step
  in a long workflow opens directly without rendering every sibling editor.
- `BatchQuickPrompt` receipts expose each child's queued, claimed,
  agent-started and settled timestamps, plus monotonic active time, calendar
  wall time and estimated machine suspension. The 10/15-minute objectives are
  explicit; a 20-minute active-time overrun fails with
  `LATENCY_BUDGET_EXCEEDED` and transactionally cancels/settles every child so
  no orphan agent can continue spending tokens.
- Planning dependencies can now be removed explicitly from the Planning UI,
  the REST API and the `kronn-internal` MCP through the narrow,
  retry-safe `task_remove_blocker` operation. It removes only the selected
  dependency edge, preserves task status and blocked reason, and records the
  acting agent when a relation actually changes.
- **Quick Exec** — runs a deterministic command (tests, typecheck, lint, PR and CI
  collection) and returns a bounded summary instead of a full log, with the streams
  kept as an artifact on disk. No shell, ever: an allowlist of binaries by bare
  name, a denylist consulted first so a shell added later is still refused, a
  literal argv, a cwd that must canonicalise inside a declared root, and an explicit
  stdin. `Passed` means exit 0 and nothing else — a timeout, a cancellation, a
  signal death and a binary that never spawned are four distinct states and none is
  a pass. A truncated stream says so on the summary's first line.
- **Review ledger** — a finding is keyed to a CAUSE, not to a comment, so five
  comments about one unwrapped error are one finding with five symptoms. The ledger
  is pinned to a head SHA, so a re-review replays only what the diff touched. A
  re-review of the two reference discussions costs 564 B per pass at the measured
  p90, against 40.9 MB for a cold pass over the whole thread.
- **Deterministic PR collection** — six templates fetch metadata, changed files,
  both comment streams, checks and reactions with no agent pass, leaving the payload
  in an artifact rather than in a context.
- **Context Architecture Audit** (`GET /api/projects/{id}/context-audit`) — what
  each of nine agent conventions actually loads in any monitored project, with
  sizes, duplication, dead references and redirect cycles. It proposes a tier split
  and never writes: a Critical section is never proposed for a move, however large.
- **RTK adoption state** (`GET /api/rtk/state`) — folds five RTK commands into one
  bounded panel. A source that cannot answer says why AND what to do; two currently
  cannot on this machine, and both are named rather than shown as empty.
- **Discussion token cost in the header** — two figures, never a total. A Kronn
  agent reports a cost per reply; a joined CLI reports a running total for its whole
  session, which also covers files read, tests run and work in other rooms. Adding
  them would double-count and misattribute.
- **CLI telemetry** — a joined CLI reports its own vendor counters. On one real
  session 4 308 007 075 tokens of traffic had been stored as `0`; unmeasured is now
  `NULL` and renders as "unknown", never as free.
- **Benchmarks and gates** — review-pass input (cold and warm), RTK compression
  floors with residual ranking, the instruction-file ratchet, and the MCP surface
  ratchet.
- **Planning parity for discussion agents** — CLI agents receive the exact
  discussion id on their first turn and can use it even when an MCP runtime
  binding is stale. Ollama and LiteLLM expose compact native Planning reads and
  writes executed inside Kronn, scoped to the current room and attributed to the
  triggering message. Vibe emits the existing human-gated proposal fence because
  its runtime deliberately runs without MCP.

### Changed

- `docs/AGENTS.md` went from 84 224 B to 13 471 B behind a ratchet that tightens on
  every gain and refuses growth.
- The manual "linked CLI session" form is gone. The binding is provenance — where a
  thread came from — established automatically at join and now shown read-only. The
  cross-agent memory API is untouched: it is what lets one agent pick up a
  discussion another started. Unlink survives alone, because a stale binding needs
  a human escape hatch.
- A silent room hands a waiting agent nothing to answer, asserted field by field.

### Fixed

- Databases opened by pre-rebase 0.9.6 builds now reconcile the former
  migration names with their final 113–119 identifiers before startup checks.
  Existing columns and tables are recognized instead of replaying their SQL
  and failing on a duplicate column.
- Three unbounded paths found by measuring rather than by reasoning: a debate
  context that sent 1 320 210 B to a model, `disc_load_other` returning whole
  discussions, and CLI sessions with no ceiling.
- Awareness backlogs are capped by bytes as well as by message count, with a
  starvation guard so one oversized message cannot block the queue behind it.
- Windows desktop builds use MSYS2's UCRT64 Pango runtime, matching Python.org
  CPython instead of mixing it with legacy MSVCRT DLLs. The docs exporter is
  now built and smoke-tested before the expensive Tauri compile.
- macOS applications are ad-hoc signed by Tauri before the DMG is assembled.
  CI verifies the uploaded image checksum and the strict signature of the app
  mounted from that image, preventing another internally invalid installer.
- The desktop shell no longer navigates to a dead random port or reuses an
  unverifiable CLI/dev/Docker listener. It acquires the shared data-directory
  lock before launch, renders a clear conflict or startup error in its embedded
  UI, and restarts the native process on Retry. Missing optional Docs-sidecar
  resources degrade without terminating the desktop shell. First boot now has
  a visible loading state and a realistic bounded startup window for database,
  keychain or antivirus initialization instead of failing after 15 seconds.


## [0.9.5] - 2026-08-11

### Changed

- Frontend lint debt is ratcheted down: all React dependency-array warnings are
  resolved, Oxlint is a zero-warning gate, and the stricter ESLint baseline is
  pinned in CI. TypeScript tooling and compatible Rust lockfile dependencies
  are refreshed to their current releases.
- Starting a discussion with several explicit `@aliases` now fans the same
  request out to independent agents. Kronn no longer turns that intent into an
  implicit debate or synthesis; agents collaborate only when the discussion
  setting allows it and one agent explicitly delegates to another.
- Multi-agent discussions explain their current collaboration policy before
  launch and link directly to the relevant discussion setting. Sibling replies
  receive bounded context so they can complement one another without repeating
  an unbounded transcript.
- Agent-to-agent delegation now uses an internal marker instead of interpreting
  ordinary generated `@alias` text as an instruction. The marker is removed
  before the response is displayed.

### Fixed

- Desktop releases once again reach the platform build matrix after dependency
  review. The security gate now names each failing backend, desktop or frontend
  audit explicitly and still completes the ordinary version-drift report before
  blocking a release; macOS packaging also opts into Tauri's deterministic
  headless DMG path explicitly. Desktop builds cache the active shared Cargo
  target and clear current plus legacy bundle paths before artifact upload.
- Duplicate root handoffs are ignored, while each explicitly targeted native
  agent still receives one durable dispatch and keeps its requested reasoning
  tier.
- LiteLLM responses no longer expose a leading private DeepSeek-style
  `<think>`/`<thinking>` block. Identically named tags later in the actual
  answer remain untouched.
- Open tabs recover cleanly after a backend restart or half-open network
  connection. The WebSocket client detects missing heartbeat acknowledgements,
  reconnects with bounded exponential backoff, ignores stale socket callbacks
  and resynchronizes the active discussion, sidebar and contact presence after
  reconnecting. A visible banner explains the temporary state without blocking
  draft editing.
- Backend availability is detected quickly without adding healthy-state noise.
  The global status retries every two seconds during an outage and probes
  immediately when the browser comes online or its tab becomes visible.
- A message is no longer considered sent before the backend confirms durable
  persistence. If the request fails before that receipt, Kronn removes the
  optimistic transcript row, restores the exact draft and surfaces the error.

## [0.9.4] - 2026-08-11

### Added

- LiteLLM is a first-class agent with encrypted endpoint credentials, live
  model discovery, independent Economy/Default/Reasoning assignments, cost
  estimates, retained configuration during VPN or proxy outages, and a model
  failure ledger with retry and removal actions.
- Ollama and LiteLLM can call Kronn's bounded native tools for MCP discovery,
  Quick APIs and Quick Prompts while credentials remain server-side.
- ccUsage discovery now supports native, Docker, macOS, Linux and legacy paths.
  Its redesigned panel filters by agent and model, separates detail and
  analysis tabs, compares token use and cost, and ranks the top two models in
  each category.
- Simplified Chinese joins French, English and Spanish as a fully separated,
  lazy-loaded interface locale. Interface language remains independent from
  the output-language context sent to non-CLI discussion agents.
- Workflow steps receive durable UUIDs distinct from editable aliases. Existing
  workflows are backfilled, imports get fresh identities and every relevant
  workflow surface exposes a consistent copy action.
- MCP planning tools accept an explicit discussion target, so agents can create
  plans and tasks in the intended room even when their runtime is not currently
  bound to it.

### Changed

- Settings now follow a clear product hierarchy: Identity, Agents, beta context
  features, Capabilities, Interface, Experience & projects, then System & data.
  Agent defaults, usage, cost, mention colours and reasoning tiers are grouped
  on the relevant agent cards with consistent contextual help and warnings.
- Skills, directives and agent profiles share one searchable capability view
  with filters for kind and Kronn/personal origin, preparing project-scoped
  import and export without conflating MCP servers with skills.
- Model and reasoning selection use the same compact picker in new discussions,
  chat aliases, Quick Prompts and workflow steps. Per-target tiers are persisted
  on each message and remain visible in the transcript instead of inheriting an
  unrelated agent's latest choice.
- New-discussion, chat and workflow prompt editors now share Markdown editing,
  preview tabs, syntax help, clickable examples, emoji completion and alias
  behaviour. Message editing uses the same responsive sizing conventions.
- The workflow wizard uses a larger workspace, sticky navigation, direct access
  to completed stages and individual steps, save/cancel actions from every
  stage, and clearer progressive disclosure for advanced step types.
- Multi-model sends reserve and order one response slot per requested agent, so
  a slow local model cannot make its placeholder disappear or place its answer
  under a later user message.
- Agent collaboration limits are expressed in user-facing terms, configurable
  globally and per discussion, with paid-agent safeguards and explicit
  transparency for unaffected CLI agents.
- Fastly and GitLab cards now document their current CLI authentication flows
  (`fastly auth login`, `glab auth login`) and visually separate credentials
  required by Kronn APIs from optional CLI-token backup fields.

### Fixed

- LiteLLM configuration and assigned tiers remain visible when the proxy or VPN
  is temporarily unavailable; models removed from the catalogue can be retired
  from the remembered failure list.
- GitLab repository discovery follows the authenticated `glab` flow instead of
  treating optional stored credentials as the only source of access.
- Kronn detects when Vibe workspace trust would silently reject the generated
  `.vibe/config.toml`, and reports the blocked directory in host sync, the agent
  card and `kronn doctor` without modifying Vibe's trust store.
- Native multi-model attribution, placeholders and turn ordering now follow the
  exact requested agent/model pair rather than falling back to the discussion
  agent or exposing native tool identities as respondents.
