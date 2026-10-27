# Three-source media references — KT-1403 (2026-10-21)

The local video recipe completed through Kronn's generation API and worker on
October 21. On October 26, the user also generated an image locally with three
references using FLUX.2 Klein 4B; the source bytes and executed graph were
checked. The separate Redux recipe remains untested because its official
weight file is absent.
[src: file: docs/research/media-references-2026-10-21.json:1]
[src: file: docs/research/media-references-klein-2026-10-26.json:1]

## Image measurement — October 26

The user submitted three distinct context files in a Kronn discussion. The
completed local ComfyUI job produced a 1344×768 image. All three uploaded files
match their sources by SHA-256; each has a `LoadImage` → resize → `VAEEncode`
path, and three ordered `ReferenceLatent` nodes feed each conditioning branch
of the executed sampler. The output is saved as a Kronn context file. A local
receipt retains the paths and hashes without committing the user's images.
[src: file: docs/research/media-references-klein-2026-10-26.json:1]

The user judged the result functional and reasonably good, with the remaining
visual outcome depending on the prompt. This is a report on this test, not a
general guarantee of identity or visual fidelity. The user's earlier test
exposed separate repeated-generation and unauthorised external-fallback
problems, tracked as KT-1481 and KT-1420; neither is fixed by this PR.
[src: user: 2026-10-26: confirmation in this conversation]

## Video measurement

Three distinct PNGs (red circle, blue square, green triangle) were submitted in
that order. Each uploaded ComfyUI input has the same SHA-256 as its source.
The executed graph has three separate `LoadImage` / `WanVaceToVideo` paths;
their conditioning is chained into the sampler and the reference latent is
trimmed before decoding. The recipe also changes the selected ComfyUI slot
after enqueueing to exercise the job's saved endpoint, family and settings.
[src: file: docs/research/media-references-2026-10-21.json:1]
[src: file: backend/tests/support/api_media_references.rs:10]
[src: file: backend/src/core/comfyui_references.rs:84]

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

## Redux and HTTP coverage

The Redux workflow may be tested separately with
`KRONN_REFERENCE_RECIPE_MODALITY=image` after installing a legitimately
obtained `flux1-redux-dev.safetensors` under `style_models/`; the vision weight
was already present on the test installation. The HTTP paths have automated
transport coverage; no billable remote generation was executed by Codex for
this qualification.
[src: file: backend/tests/support/api_media_references.rs:10]
[src: file: backend/src/api/comfyui.rs:328]

See [the media architecture](../architecture/media-generation.md#configurable-source-image-limits-kt-1403)
for the supported workflow matrix, dependencies and exact recipe command.

## October 26 rebase for 0.15.2

The branch now builds on `main` at `9f3889ee2`. KT-1418's exact-filename
lookup is preserved: accepted references are stored as canonical context-file
ids, and aliases of one image count once toward the model's visual-reference
limit. The API/worker regression mixes filenames (including an accented name)
and ids for three sources, for both image and video, then changes the local
slot before the worker runs.
[src: commit: 9f3889ee2]
[src: file: backend/src/api/media.rs:1877#resolve_references]
[src: file: backend/tests/api_tests.rs:27884#media_generate_accepts_an_exact_filename_of_this_rooms_image_only]
[src: file: backend/tests/support/api_media_references.rs:149#comfyui_three_sources_survive_a_slot_change_between_the_api_and_worker]

The old CI catalogue failure was a 220-byte increase in the media tool
declaration. Compact guidance retains the source, ordering, limit and slot
contracts; the measured catalogue is now 84,389 bytes, 61 bytes below this
base's ceiling. The ceiling is lowered to that measurement.
[src: file: backend/scripts/ci/mcp_surface_budget.py:154#CATALOGUE_MAX_BYTES]
[src: file: backend/scripts/disc-introspection-mcp.py:2378]

The October 21 video evidence is historical; no new real generation was run
for this rebase. The user subsequently ran the Klein image measurement above;
the separate Redux workflow remains untested.
[src: file: docs/research/media-references-2026-10-21.json:1]

## October 21 automated validation

Before rebasing on `36de741e4`, the full nextest run passed 11,277 functional
tests; its only failure was displaced documentation citations. The complete
frontend suite passed 6,817 tests, with the four local-server tests rerun
outside the filesystem/network sandbox after their port binding was refused.

After the October 21 rebase, these checks passed:

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
servers; full browser E2E and a live remote provider run were not performed for
that October 21 validation. See the later Klein measurement above.
