# Local ComfyUI media — measurements (2026-10-19)

Machine: MacBook Pro M5 Max, 64 GB unified memory. Comfy Desktop 1.1.7 (Homebrew
cask `comfy`), ComfyUI 0.42.1, standalone env `mac-mps` (Python 3.13.12, torch
2.12.1), device `mps`. Ticket KT-1381.

## Why ComfyUI

Ollama 0.40.6 pulls `x/flux2-klein:4b` (`/api/show` capabilities `["image"]`) but
`/api/generate` answers `400 image generation models are not currently
supported`: Ollama removed its MLX image generation on 2026-07-28
(ollama/ollama#16615, "to be re-introduced on the new MLX runner").

## Install

- macOS: `brew install --cask comfy` (verified). First launch is a GUI wizard
  (GPU, install folder, terms the user accepts himself), then the server listens
  about a minute later on `http://127.0.0.1:8188`, announced in
  `~/ComfyUI-Installs/ComfyUI/logs/comfyui.log`.
- Windows `winget install Comfy.ComfyUI-Desktop` and Linux
  `uvx --from comfy-cli comfy --skip-prompt install --nvidia`: found in the
  official package sources, NOT run.

## API used

| Call | Use |
|---|---|
| `GET /system_stats` | up? + `system.comfyui_version` |
| `GET /models/<folder>` | installed files (`checkpoints`, `diffusion_models`, `text_encoders`, `vae`); a new file shows without restart |
| `GET /view_metadata/<folder>?filename=` | safetensors header; FLUX declares `modelspec.architecture = Flux.1-schnell`, RealVisXL has none (404) |
| `POST /prompt` `{"prompt": <workflow>, "client_id": "…"}` | queue → `prompt_id`; a refused graph answers 400 with `error` + `node_errors` |
| `GET /history/<prompt_id>` | `{}` while running, then `status.completed` + `outputs`; failure = `status_str: "error"` |
| `GET /view?filename=&subfolder=&type=output` | the produced file (Desktop does not write under `ComfyUI/output`) |
| `POST /upload/image` (multipart `image`, `overwrite`) | input for img2img |
| `POST /queue {"clear": true}` + `POST /interrupt` | stop work (killing the client does not) |

## Results

| Run | Settings | Time | Verdict |
|---|---|---|---|
| FLUX.1 schnell fp8 (17,236,328,572 B) | 1024², 4 steps, cfg 1, euler/simple, empty negative | 40.9 s cold, 10.8 s warm | clean, polished "AI picture" look |
| FLUX.1 schnell, camera prompt | 1216×832 | 50.8 s | more photographic, still smoothed |
| RealVisXL V5.0 fp16 (6,938,065,488 B) | 1216×832, 30 steps, cfg 5, dpmpp_sde/karras, recommended negative | 64.1 s cold, 54.6 s warm | clearly photographic; garbled text, weak logic (two crossing katanas) |
| FLUX → RealVisXL img2img | FLUX 1344×768 (26.7 s), then denoise 0.45 / 0.6 | 121 s / 107 s | best: FLUX's coherence with a photographic render |
| Wan 2.1 T2V 1.3B fp16 + umt5_xxl fp8 scaled + wan_2.1_vae (~9.8 GB) | 832×480×33, 30 steps, cfg 6, uni_pc/simple, shift 8, 16 fps | 594 s (≈12.8 s/step) | valid MP4 H.264 High, 2.06 s, content unusable (colour blobs); quality is KT-1382 |

Prompting that read as a film still: "Film still from a live-action movie,
photorealistic … realistic creature visual effects like Planet of the Apes …
shot on ARRI Alexa 65, anamorphic lens, cinematic colour grading, film grain";
for SDXL a negative "cartoon, illustration, 3d render, cgi, plastic, toy…".
