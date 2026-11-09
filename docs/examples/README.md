# Examples

Workflow files you can import into Kronn as they are. Each one is a workflow
export (`"kind": "kronn.workflow"`, version 4), the format
`GET /api/workflows/:id/export` writes and `POST /api/workflows/import` reads.
`cargo test --lib docs_example_workflows_import` imports every file here
through the real import route, so CI keeps them valid.

## Design system first visual

Creates the project's first visual from its design system, in four Agent
steps: ① reads the design tokens and brand guidelines and writes an image
prompt, ② checks the prompt against the brand, ③ generates the image with the
brand assets as reference images, ④ turns the image into a short teaser
started from that image. The three files are the same four steps on three
lanes of engines.

| File | ① Write | ② Review | ③ Image | ④ Video |
|---|---|---|---|---|
| [`design-system-first-visual.cloud.json`](design-system-first-visual.cloud.json) | Claude Code, Sonnet | Codex | LiteLLM or OpenRouter | OpenRouter |
| [`design-system-first-visual.local.json`](design-system-first-visual.local.json) | Ollama, Default tier | Ollama, Advanced tier | Ollama → ComfyUI | Ollama → ComfyUI |
| [`design-system-first-visual.hybrid.json`](design-system-first-visual.hybrid.json) | Claude Code, Sonnet | Ollama, Advanced tier | Ollama → ComfyUI | Ollama → OpenRouter |

In the cloud file, steps ③ and ④ run on an Economy-tier Claude Code that
calls `media_generate` through the MCP bridge; in the two others, an Ollama
step calls it natively. A workflow Agent step on an HTTP or local agent has
`media_generate` and `media_job_status` and no other orchestration tool: its
asset lands in a room Kronn opens for the run and, once ready, in the run's
artifacts directory, and it waits with `media_job_status` and `wait_secs`.
See [HTTP agent capabilities](../architecture/http-agent-capabilities.md).
The local file keeps every model and every pixel on your machine. The hybrid
file sends the prompt work to Claude Code, keeps the review and the image
local, so the logo and palette files never leave the machine, and sends only
the generated image to a cloud video model.

### Import

Automation › Import, drop the file and pick the project whose
repository holds the design system. The workflow arrives disabled with a
manual trigger; it asks for its variables at each launch.

### What to configure

- **The design-system paths.** `design_system_paths`: the tokens and the
  brand guidelines (`design/tokens.json, docs/brand-guidelines.md`),
  relative to the repository root.
- **The reference images.** `brand_asset_paths`: repository paths of the
  logo and the palette sample, logo first, as png, jpg, webp or gif files.
  Cloud: steps ③ and ④ join the room given as `room_id`, attach them there
  and generate from them. Local and hybrid: the Ollama step passes the paths
  as `reference_asset_ids`; Kronn reads each file inside the project (no `..`,
  no absolute path, no link) and attaches it to the run's room. Step ④ then
  names the image by the job id step ③ returned, a generation of the same
  run.
- **Connections.** `image_connection` and `video_connection` take a
  connection's id or alias, as Config › Agents names it (cloud: both;
  hybrid: `video_connection`). The image connection needs an image slot whose
  model accepts reference images; the video one a video slot whose model lists
  `first_frame`. See
  [media generation](../architecture/media-generation.md).
- **Local models.** The Ollama Default and Advanced tiers are two different
  models (`qwen3:8b` and `qwen3:30b-a3b` unless you changed the tiers). Pull
  them, or set `agent_settings.model` on each step.
- **ComfyUI.** Config › Agents › Local › ComfyUI: an Image model whose Kronn
  workflow takes source images (FLUX.2 Klein 4B up to 4, FLUX.1 schnell with
  Redux up to 3, SDXL img2img 1) and, for the local video, a Wan 2.1 VACE
  model with its text encoder and VAE, which takes the image as a reference
  rather than a first frame. Local video needs a very powerful machine; it is
  close to impossible even on a good Mac.

Steps ③ and ④ never generate without the brand images: when the model
cannot take them, the cloud steps see it in `agent_list` and Kronn refuses
the local and hybrid steps' request before anything is billed; the step
then stops and says why. A run starts at most 12 generations
(`KRONN_WORKFLOW_MEDIA_JOBS_PER_RUN`), and a dry-run Test generates nothing.
