# Releasing

A release is created by the **Desktop Build** workflow, never by hand: a
release published by hand has no installer. 0.12.0 to 0.14.1 shipped that way,
and the update banner led users to empty release pages (KT-970).

## Steps

1. `make docs-reanchor`: it re-points every doc citation whose `#symbol`
   moved, when the symbol occurs once in the cited file, then runs the strict
   citation check over every doc. Fix what it lists by hand and commit. A pull
   request that changes `VERSION` gets that strict check in CI too.
2. `make bump V=x.y.z`, update "What's new" in `README.md` and the site note
   (`make bump` leaves them), then run `scripts/check-version-sync.sh`
   directly.
3. Merge, then tag the merge commit `x.y.z` (no `v`) and push the tag.
4. Desktop Build first runs `scripts/check-version-sync.sh <tag>`: the tag must
   equal `VERSION` (a `-rc1` tag fails) and every version marker must agree.
   It then builds the four installers (Windows `.exe`, macOS Apple Silicon and
   Intel `.dmg`, Linux `.deb`: exactly the Tauri targets `deb`, `nsis`, `dmg`;
   no `.msi` or `.AppImage`), checks that each is a real, non-empty file, and
   creates a **draft** release carrying them.
5. Its last step reads the release's assets back from GitHub and fails if a
   platform has no installer (`verify_artifacts.py --release-assets`).
6. Review the notes and publish the draft.

Desktop Build resolves the release commit once (`release-checks`, output
`sha`) and every checkout uses that SHA, so a manual dispatch whose branch and
tag differ still builds what it ships. It reruns no test suite: the merged pull
request was the gate, with `ci-quality-gates` and `ci-build-gates` both
required; `ci-test.yml` and `ci-build.yml` never run on a tag. A tag only checks
the version, builds and verifies the installers, and creates the draft.
A `workflow_dispatch` with an empty `release_tag` is a build-only dry run.

## A release that already exists without installers

Run Desktop Build by hand (`workflow_dispatch`) with `release_tag` set to the
tag. The installers are built from that tag, attached to the existing release
without touching its notes or status, then verified the same way. The checks
run from that tag, so it must already carry `scripts/check-version-sync.sh`:

```bash
gh workflow run "Desktop Build" --ref feat/x.y.z -f release_tag=0.14.1
```

## Update banner

The desktop app sets `KRONN_DESKTOP_APP=1`. The banner then only offers a
release that has an installer for the user's platform. Docker and source
installs update from the tag, so the banner offers them every release.

## Docker images (GHCR)

Pushing the tag also runs **Docker Images** (`.github/workflows/docker-images.yml`).
It builds the backend (`CARGO_PROFILE=release`) and the frontend on native
runners, one per architecture (`ubuntu-24.04` for amd64, `ubuntu-24.04-arm` for
arm64; the Rust backend is never built under QEMU), then ties both under one
multi-arch tag: `ghcr.io/docroms/kronn-backend:<version>` and
`ghcr.io/docroms/kronn-frontend:<version>`, plus `latest` when the version is the highest stable release tag. `latest` is moved by a separate job (`latest`) that runs one at a time per image (`concurrency: docker-images-latest-<service>`, `cancel-in-progress: false`) and, inside that group, points `latest` at the highest stable release tag whose image is published: a slower older run, or two overlapping releases in any order, end with `latest` on the newest one. A newer tag whose images the registry confirms are not published yet ("not found") leaves `latest` where it is until its own run publishes; any other registry or `git ls-remote` error (network, auth, 5xx) is retried and then fails the job; republish a failed one with `workflow_dispatch`. `make start` pulls the
tag named by `VERSION`, so the tag and `VERSION` must agree (the job runs
`scripts/check-version-sync.sh <tag>` first), and `docker-compose.yml` carries
that version as its default image tag (`make bump` updates it).

- **One-time, after the first run**: GitHub creates the two packages private.
  Open each package's settings (Packages > `kronn-backend` / `kronn-frontend` >
  Package settings > Danger Zone > Change visibility) and set it **Public**; the
  API cannot do it. Until then an install cannot pull and falls back to the 19 GB
  source build. The `verify-public` job fails, naming this step, while a package
  is still private.
- **Republish a tag**: run Docker Images by hand (`workflow_dispatch`) with `tag`
  set to the release, e.g. `gh workflow run "Docker Images" -f tag=0.15.1`. The
  application source comes from the tag; `backend/Dockerfile` and
  `backend/entrypoint.sh` come from the commit the workflow runs at, so an old tag
  still gets the runtime UID/GID remap (an older Dockerfile would pin uid 1000).
