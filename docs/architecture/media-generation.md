# Media generation (image / video)

HTTP connections (LiteLLM, NVIDIA, OpenRouter) can generate images and videos
in addition to serving chat. This page states the model, the invariants that
cost money if broken, and the entry points.

## Modalities, not tiers

A connection carries `economy_model` / `default_model` / `reasoning_model` for
text, and separately `image_model` / `video_model` for media. Media are a
different **axis**, not extra `ModelTier` variants: adding `Image` to the tier
enum would let a text step select "tier Image", and every tier match site would
have to grow a branch that means nothing.

`media_endpoint` overrides the host media is served from. NVIDIA needs it: its
connection stores `integrate.api.nvidia.com` while visual models answer on
`ai.api.nvidia.com`.

The model is always read from the slot, never from the caller — an API or UI
client cannot bill a model the operator did not configure.

The slots themselves are selected from the shared capability-bearing model
catalog after a successful connection test. OpenRouter is queried through its
three real catalog routes: chat (`/v1/models`), image
(`/v1/images/models`) and video (`/v1/videos/models`), and its `/v1/models`
entries carry `architecture.output_modalities` — real per-model evidence. The
Image picker only shows records confirmed with `image`; Video only records
confirmed with `video`. A formerly saved id that is no longer detected stays
visible as unavailable, but is neither presented nor persisted as a detected
capability.

NVIDIA's `/v1/models` carries none of that: every entry — image and video
models included — comes back shaped exactly like a chat model. Treating an
undeclared modality as "not media-capable" hid every real NVIDIA image/video
model behind a filter that was always empty (KT-531). The picker instead
reads catalog evidence per connection — `image_capability_known` /
`video_capability_known` on the test response, true only when
`output_modalities` was actually seen or a dedicated capability endpoint
answered — and renders one of three states, never a name/vendor/id-prefix
guess:

* **known** — filter strictly to the declared capability (OpenRouter, and any
  future provider whose catalog does the same);
* **unknown** — no modality evidence at all: show the full catalog behind an
  explicit warning, keep search and free-text entry, never touch a saved
  value;
* **unsupported** — the catalog does declare capabilities and none match: the
  slot has no compatible model, stated plainly instead of an empty dropdown.

## Not OpenAI-compatible

`MediaCodec` (`backend/src/agents/media_codec.rs`) is a separate trait from
`ChatCodec`, and its methods return **complete URLs** rather than a path
appended to one base, because the media host can differ from the chat host.

* OpenRouter images: `POST /api/v1/images`, synchronous, base64 in
  `data[].b64_json`.
* OpenRouter videos: `POST /api/v1/videos` answers `202` with a handle, then
  polling until `completed`, then a content URL.
* NVIDIA: its own visual routes, results under `data[]`.

Codec tests replay captured real payloads. Provider error text is kept but
bounded to 300 characters so a raw payload never reaches storage or the UI.

## Invariants that cost money

**A billable POST is never sent twice.** `submit_attempted_at` is stamped and
committed *before* the request leaves. If the process dies in flight, recovery
finds that mark with no handle and refuses to resubmit
(`MediaAction::RefuseUnsafeResume`): the provider may already be generating and
charging, and nothing afterwards can tell us whether the request arrived. An
explicit failure the human can retry beats a silent double charge.

**A claimed job is claimed once.** `media_jobs` reuses the `agent_resume_jobs`
pattern — due-selection, atomic claim, orphan reclaim — deliberately not the
delegated-task lifecycle, which assumes a worktree, a review and a commit.

**The deadline outranks everything.** `next_action` checks it first; an expired
job is settled and published explicitly, because an expired row stays `pending`
and `due()` would never surface it again.

**Cost is persisted verbatim.** `usage.cost` and `is_byok` from the provider,
for both modalities, never recomputed from a published rate — measured drift on
one clip: 0.0708932 USD billed against 0.0678 implied. BYOK bills zero here
while the real spend sits upstream, so the flag travels with the figure and
BYOK rows are excluded from estimates.

**Geometry is read from the produced file.** A "480p 16:9" request came back as
864×496, so the requested parameters describe nothing (`core/media_probe.rs`).

## Provider URLs are untrusted input

A completed generation answers with URLs we did not write. Every one goes
through `validate_asset_url` (`backend/src/agents/media_asset_url.rs`) before
being fetched:

* the configured endpoint's **origin** (scheme + host + port) is trusted as the
  operator's own choice, so a self-hosted `http://127.0.0.1:4000` keeps working
  — but only that port, not its neighbours;
* any other host must be `https`, on the standard port, not a raw IP, without
  userinfo, and vouched for by the codec's `asset_host_policy`;
* the policy separates `credentialed` hosts from `anonymous` ones: on
  pre-signed storage the URL *is* the authorisation and the Bearer is not
  attached.

An unknown asset host fails the job naming that host rather than fetching from
it silently.

## Where the asset lands

The asset is downloaded server-side and stored as a context file, so no signed
provider URL is ever handed to a browser. Each launch in an existing discussion
creates its own durable media anchor message. A caller-provided `message_id` is
a causal source (`reply_to_message_id`), not an anchor to reuse: concurrent
generations started from one prompt therefore keep distinct transcript
positions and run identities.

The anchor is rendered as one media bubble whose state changes in place from
pending/running to success or failure. On success it shows the media itself,
fetched through the authenticated context-file endpoint as an object URL, and
offers a single action — opening that exact persisted asset in the canonical
Assets viewer. The shared run card's generic "open the run" link is suppressed
here: the card rebuilds its own model on every rehydration, so the suppression
is a prop of the card (`hideRunLink`), never an `href: null` the next
rehydration would overwrite. The Assets entry remains available when a
discussion has no file yet, because it is also the entry point for the first
generation.

The bubble shows what a reader asked for and nothing else: the raw job JSON is
folded behind a "Details" toggle, the price sits at the bottom right once
billed, and the only duration on screen is the length of a produced clip — a
picture has none, and an elapsed run time there read as "this media lasts 8s".
Every one of these rules is scoped to `kind === 'media'`; workflow and
quick-prompt cards keep their existing rendering.

Completion emits `ContextFilesChanged`, so an open discussion shows the asset
without a reload.

## Entry points

* `POST /api/media/generate`, `GET /api/media/jobs/{id}`,
  `POST /api/media/jobs/{id}/cancel`, `GET /api/media/costs`,
  `GET /api/media/estimate`.
* UI: the discussion's **Assets** panel has three tabs. **Tout** is the
  inventory (search, filters, viewer). **Creator** is the launcher (modality,
  connection, prompt, duration / resolution / ratio, soundtrack, estimated
  price); it is always offered, and with no media slot configured the form
  itself says so. **Editor** appears from the second clip on: it orders the
  clips and plays them as one film (see below).
* Agents: `media_generate` and `media_job_status`, by two independent routes —
  CLI agents through the `kronn-internal` MCP bridge, HTTP agents through the
  native orchestration catalogue
  [src: file: backend/src/api/agent_tools.rs:420-445]. The bridge route came
  first; the native one closed the half of the fleet that could not reach it.
* **The connection is optional (KT-686).** A generation names one, or names
  none and Kronn uses the only connection configured for that modality. When
  several can serve it, the request is refused with their names: choosing
  between two connections is choosing whose budget is spent, and Kronn never
  does that for a human. The answer says which connection was billed.
  Measured on six local models before and after: `gemma4:e2b` and `gemma4:e4b`
  called no tool at all and asked the human for an id they could not know —
  with the whole catalogue declared, `agent_list` among it. `connection_id`
  was the only parameter of `media_generate` with no description, and it was
  required; a model reads the schema and treats such a field as caller input.
  The same reasoning as `api_call`, whose config id became optional after a 4B
  model paired a plugin with another one's credentials. Omitting the id and
  naming it are the same intention, so they produce the same job: the v2
  idempotency digest excludes the connection on purpose.
* Discovery: an agent reads which modalities are available from the worker
  catalogue `agent_list` returns — one entry per configured model, absent when
  nothing is configured, so availability is never asserted on faith. Each entry
  carries the same envelope the launcher reads (`capabilities`: durations,
  resolutions, ratios, frame positions), so an agent picks a duration the model
  named instead of discovering the list from a refusal.
* Publication goes through the single point
  `api::shared_runs::publish_media_job` — persisting the run and broadcasting
  it are inseparable, so a 100 s generation is visible while it runs.

### New discussion entry

New discussion exposes its media entry only when an external connection has a
configured image or video slot. Selecting the entry only mounts the existing
media form; it neither creates a room nor dispatches an agent. The form creates
a normal `no_agent` discussion at an explicit Generate click, then passes that
room to the existing generation endpoint. A successfully created room is kept
across a generation error, and the same idempotency key is retained for an
explicit identical retry; the UI never automatically repeats an ambiguous
billable request. `[src: file: frontend/src/components/NewDiscussionForm.tsx:227-231]`
`[src: file: frontend/src/components/MediaGenerateForm.tsx:476-532]`
`[src: file: frontend/src/pages/DiscussionsPage.tsx:2363-2379]`

## What a model accepts, read rather than assumed

`GET /api/media/models?connection_id&modality` reads the provider's own
catalogue for the model configured on that connection, and the launcher offers
exactly what it names. The launcher used to offer 3 s and 1080p against
`bytedance/seedance-2.0-mini`, which accepts 4..15 s and 480p/720p only — two
billable clicks that could only come back rejected — while hiding three of the
seven ratios it does accept.

The two OpenRouter catalogues have different shapes: the video route is flat
(`supported_durations`, `supported_resolutions`, `supported_aspect_ratios`,
`supported_frame_images`, `generate_audio`), the image route nests typed
entries under `supported_parameters` (`aspect_ratio` as an enum,
`input_references` as a range). An unadvertised field stays empty and an empty
field means "do not offer it". A provider with no media catalogue at all
(NVIDIA serves none) leaves the launcher on its configured fallbacks rather
than emptying it. Answers are cached in memory for ten minutes.

A value the envelope explicitly excludes is refused at `/api/media/generate`,
with the list, before the provider is called — the same rule the reference-mode
preflight already applied, extended to duration, resolution and ratio. Silence
is never a refusal: an unreadable catalogue or a field the provider never
advertises leaves the submission to decide. Resolutions and ratios are compared
trimmed and case-insensitively, so a caller is never refused over `720P`.

The same read serves both surfaces. `agent_list` sweeps every configured media
slot concurrently under one 8 s window and attaches what came back
[src: file: backend/src/api/orchestration.rs:8547-8588]; the catalogue builder
itself stays pure and is handed the result, like the reachability and quota
preflights beside it. An envelope is attached only when the model it was read
for is still the configured one — a connection whose model changed between the
sweep and the build gets silence, never the previous model's durations. A
timeout or an unreadable catalogue leaves `capabilities` null rather than an
empty envelope, whose empty lists would read as "this model supports nothing".

Deliberately NOT stored in `model_catalog_entries` (KT-531): that table is a
catalogue of AGENT models, with provenance and tier assignment. These envelopes
are per provider model and volatile — a max duration changes without notice —
and persisting them would make a stale row authoritative over the provider.

## Chaining a clip into the next one

A reference is always a picture, so continuing a clip means passing that clip's
last image. Kronn cannot produce it: the clips these providers return are H.264
High profile, and the pure-Rust decoder available server-side reads nine frames
out of ninety-seven before failing — an extractor written without that check
would return the ninth frame as "the last image", sharp and wrong (KT-550,
spike kept in `.kronn/research/kt550-frame-spike/`). ffmpeg is on neither the
machine nor the repo.

The browser decodes them without trouble, so KT-556 puts the extraction in the
viewer: the reader keeps the last image from the Assets carousel and it lands in
the room as an ordinary context file. An agent cannot do this on its own, so
every surface that could mislead it now says who to ask — the `media_generate`
declarations on both routes, the `tool_manual({tool: "media_generate"})` page,
and the refusal returned when a clip id is passed as a reference.

Not to be confused with `reference_mode: "last_frame"`, which is a generation
constraint — the new clip must END on the picture supplied — and not an
extraction.

## Generating from images already in the room

The Assets carousel may hand its currently viewed image to the existing media
form for a compatible configured slot. This is an explicit handoff: it closes
the viewer and switches to the Creator tab with the source attached, but never
starts a generation on its own. Until the selected model's input capability resolves,
the preserved source blocks submission rather than being silently downgraded
to a text-only paid request.

A video may start from — or end on — a picture the discussion already holds,
and an image may be drawn from several of them. The request names them by
context-file id (`reference_asset_ids`) plus a mode (`first_frame`,
`last_frame`, `reference`); it never carries a path or a URL, so neither a
browser nor an agent learns where a file lives or can point a generation
outside the room. The single-id form is still accepted and still read on jobs
recorded before several pictures were possible; it is never written again.

Everything is checked before anything durable is written and long before the
provider is paid: the asset exists, belongs to THIS discussion, is an image
with stored bytes, is under the size ceiling, and its mode is one the model
advertises. A refused request leaves no anchor message and no job. A catalogue
that cannot be reached does not become a refusal — the provider stays the
authority — but a mode it explicitly does not list is refused here.

Only the id is persisted, in `media_jobs.params_json`. The worker re-reads the
file on each attempt, so a job resumed after a restart uses the same picture
and one that left the discussion in between fails the job instead of reaching
the provider stripped of what made it the requested generation.

The images travel to the provider as `data:` payloads. Kronn listens on
127.0.0.1: there is no URL a provider could fetch, and publishing one would
hand a private file to the internet. The documented shapes are
`frame_images: [{type, image_url: {url}, frame_type}]` on the video route and
`input_references: [{type, image_url: {url}}]` on the image one — the same
object without the frame. Order is preserved: providers weigh references by
position.

How many references an image model takes is its own: the catalogue states it
as a range and it runs from 1 to 16 across the models, so the form offers what
each one advertises and the request is refused above a stated maximum. A
catalogue that advertises nothing stays silent and lets the submission tell.

The finished result states what it was built on: the run projection echoes the
source ids and the mode, and the bubble shows a line — "starts on an image of
this discussion" — whose links open those pictures in the viewer. Ids, resolved
through the discussion's own files; a mode the reader cannot name is dropped
rather than labelled with an invented word.

## Taking the last frame of a clip

A clip's last picture is extracted in the BROWSER, from the viewer's own
player, and uploaded as a context file of the discussion — from there it is a
starting picture like any other. The backend cannot do this: the clips these
providers return are H.264 profile 100, which the pure-Rust decoder reads for 9
frames out of 97, and ffmpeg is on neither the machine nor the repo. The
browser decodes them, and the action only ever runs from a click in the viewer,
so the element that already succeeds does the work — with no dependency and no
divergence between Docker, Tauri, macOS, Linux, Windows and WSL.

The extraction proves it decoded something — non-zero dimensions and a
non-uniform canvas — and fails with a sentence otherwise. A clip that genuinely
ends on a fade to black is refused too: a visible refusal costs a retry, while
a black rectangle passed off as the last frame is paid for as the starting
picture of the next generation.

Four deliberate refusals. A visual reference on a video is refused rather than
submitted as a frame: no video model advertises `reference` under
`supported_frame_images`, and passing it as a frame would have the model
reproduce a picture that was only an inspiration — and bill for it. A frame on
the image route is refused symmetrically: an illustration has no first or last
one, and demoting the request to a plain reference would bill a generation
nobody asked for. Several images for one frame are refused whether or not a
catalogue answered, since keeping one of them silently is the same charge for
something else. NVIDIA refuses source images naming what is missing, because
neither its image-to-video nor its reference contract has been measured here
and an invented payload would be billed on a supposition.

## The soundtrack is a decision, never a default

Providers add audio to a video unless told otherwise. `generate_audio` reaches
them through `MediaParams`, but an absent field left that decision to the
provider — which is how a clip came back refused for audio copyright with
nothing in the request to explain it. Both callers now state the value they
mean: the form sends it for every video, and `media_generate` fills in `true`
when the agent says nothing, with the default written in the tool description
so an agent knows it can ask for silence. An image is never asked the question.

Media spend is its own counter: a generation is billed per image or per second
and its usage payload carries no token count at all, so folding it into the
token counters would report zero tokens against real spend.

## Playing the clips as one film

A video model produces clips of a few seconds, so a longer film is several
clips played back to back. The Editor tab of the Assets panel says which clips
make the film, in what order, and plays them through.

* **The order belongs to the discussion.** `discussion_video_sequences` keeps,
  per discussion, the ordered list of context-file ids that make the film
  (migration 182) and the list of clips set aside from it (migration 183),
  read and replaced together through
  `GET` / `PUT /api/discussions/{id}/video-sequence`. A write is refused when
  it names a file of another discussion, or the same clip twice, in either
  list. A deleted clip drops out when the order is read, not when it is
  deleted, so a deletion never needs to know about the sequence.
  [src: file: backend/src/db/discussion_video_sequences.rs]
* **A new clip joins the film, at the end.** Clips nobody has placed yet
  follow the arranged ones, oldest first: clips are generated one after the
  other, so the order they were made in is the film's default. A clip that
  has nothing to do with the film is dragged to "Hors version finale", or
  taken out with its button: it stays in the discussion and is not played,
  until it is put back.
  [src: file: frontend/src/lib/videoSequence.ts]
* **Every move is saved at once.** Drag-and-drop or the buttons send both
  lists; a refusal is shown in place, and the list keeps the order on screen.
  While a clip is dragged, a line between two rows shows where it will land
  (the upper half of a row points above it, the lower half below), and none
  is drawn where the clip would not move.
  [src: file: frontend/src/lib/videoSequence.ts]
* **Each clip shows its first frame.** The thumbnail is the clip itself,
  paused near its start, fetched once its row is on screen. The bytes are
  fetched once per clip and shared with the player.
* **Each clip shows its length and price, and the film its totals.** A
  generated clip's provenance (`ContextFile.ai_generation`) carries the length
  read from the file's header (`media_jobs.rendered_duration_ms`) and the price
  the provider declared (`media_jobs.cost_usd`, with `is_byok`). An uploaded
  clip's length is read by the browser from the clip itself. The totals count
  only the clips of the film. A clip with no declared price, an upload, or one
  billed on the user's own key is not added: the total says how many were left
  out rather than counting them as free.
  [src: file: frontend/src/lib/videoSequence.ts]
* **"Lire la version finale" plays, it does not render.** A full-screen player
  shows each clip of the film in order and starts the next one when the
  current one ends,
  fetching the following clip while the current one plays. A clip that cannot
  be loaded is announced with a button to skip it. No file is produced:
  cutting and exporting a single video are separate work.
  [src: file: frontend/src/components/VideoSequenceEditor.tsx]
