# Three-source media references — KT-1403 (2026-10-21)

The local video recipe completed through Kronn's generation API and worker.
The image recipe remains pending: the official Redux weight file is absent and
requires the user's Hugging Face access. This is a partial qualification, not
an end-to-end image-and-video acceptance.
[src: file: docs/research/media-references-2026-10-21.json:1]

## Video measurement

Three distinct PNGs (red circle, blue square, green triangle) were submitted in
that order. Each uploaded ComfyUI input has the same SHA-256 as its source.
The executed graph has three separate `LoadImage` / `WanVaceToVideo` paths;
their conditioning is chained into the sampler and the reference latent is
trimmed before decoding. The recipe also changes the selected ComfyUI slot
after enqueueing to exercise the job's saved endpoint, family and settings.
[src: file: docs/research/media-references-2026-10-21.json:1]
[src: file: backend/tests/support/api_media_references.rs:10]
[src: file: backend/src/core/comfyui_references.rs:80]

| Measurement | Observed value |
| --- | --- |
| Model | `wan2.1_vace_1.3B_fp16.safetensors` |
| Kronn job | `bb0095cc-2144-43ac-9f13-a248c5334940` |
| ComfyUI prompt | `0b1f5dc0-b8ee-4dda-91a7-9f63419b2df5` |
| Sampling | 8 steps, cfg 6, uni_pc/simple, shift 8 |
| ComfyUI execution | 702.533 seconds |
| Result | H.264 MP4, 624×624, 33 frames at 16 fps, 2.0625 seconds |
| Result size | 89,312 bytes |
| Result SHA-256 | `a2b2625c2db448215aae1829a54cf3bb652ff29d02407f3f7cda029e4973e401` |

The [machine-readable evidence](media-references-2026-10-21.json) preserves
the request, source hashes, job receipt and complete executed graph/history.
Local source images, MP4 and frames 0/16/32 are retained under
`.kronn/artifacts/kt1403/video/` in the main checkout; binaries are not committed.
[src: file: docs/research/media-references-2026-10-21.json:1]

Visual inspection of those frames shows a nearly static result: a red circular
shape and triangular forms appear, but the blue square is not faithfully
reproduced. This run verifies three-source transport and execution. It does
not qualify reference fidelity, identity preservation or production video
quality. Eight steps were used for this transport recipe; no comparison with
the default step count was performed.
[src: file: docs/research/media-references-2026-10-21.json:1]

## Remaining acceptance

Run the same opt-in recipe with `KRONN_REFERENCE_RECIPE_MODALITY=image` after
installing the legitimately obtained `flux1-redux-dev.safetensors` under
`style_models/`. The vision weight is already present on the test installation.
Keep the image and video acceptance items open until the image output and its
three-source graph are available. The HTTP paths have automated transport
coverage; no billable remote generation was executed for this qualification.
[src: file: backend/tests/support/api_media_references.rs:10]
[src: file: backend/src/api/comfyui.rs:322]

See [the media architecture](../architecture/media-generation.md#configurable-source-image-limits-kt-1403)
for the supported workflow matrix, dependencies and exact recipe command.

## Automated validation

Before rebasing on `36de741e4`, the full nextest run passed 11,277 functional
tests; its only failure was displaced documentation citations. The complete
frontend suite passed 6,817 tests, with the four local-server tests rerun
outside the filesystem/network sandbox after their port binding was refused.

After rebasing, these checks passed:

- `cargo nextest run -E 'test(media) | test(comfy)' --test-threads 6`: 214 tests.
- `cargo clippy --tests -- -D warnings`, `cargo fmt --check`, `make typegen`.
- The strict whole-repository documentation citation test, after repairing the
  moved citations; the separate mechanical commit changes citation line numbers
  only in 30 documents.
- Four affected frontend test files: 61 tests; after the final model-change
  retry correction, the form's 33 tests passed again, including its new case.
- TypeScript build, ESLint and Oxlint on the changed components, and i18n lint
  (6,795 keys in four languages; existing unused-key warnings remain).
- The Python MCP reference/audio contract suites: 12 tests.
- `lint_with_conn_unwrap.py` and `git diff --check`.

The real ComfyUI video recipe passed separately. HTTP provider tests use mock
servers; full browser E2E and a live remote provider run were not performed.
The image recipe remains the explicit acceptance gap described above.
