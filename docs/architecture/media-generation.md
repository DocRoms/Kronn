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
(`/v1/images/models`) and video (`/v1/videos/models`). The Image picker only
shows records confirmed with `image`; Video only records confirmed with
`video`. A formerly saved id that is no longer detected stays visible as
unavailable, but is neither presented nor persisted as a detected capability.
NVIDIA model records follow the same output-capability parsing contract.

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
* UI: the discussion's **Assets** tab has a launcher (modality, connection,
  prompt, duration / resolution / ratio, soundtrack, estimated price).
* Agents: MCP `media_generate` and `media_job_status`.
* Publication goes through the single point
  `api::shared_runs::publish_media_job` — persisting the run and broadcasting
  it are inseparable, so a 100 s generation is visible while it runs.

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
