# Releasing

A release is created by the **Desktop Build** workflow, never by hand: a
release published by hand has no installer. 0.12.0 to 0.14.1 shipped that way,
and the update banner led users to empty release pages (KT-970).

## Steps

1. `make bump V=x.y.z`, update "What's new" in `README.md` and the site note
   (`make bump` leaves them), then run `scripts/check-version-sync.sh`
   directly.
2. Merge, then tag the merge commit `x.y.z` (no `v`) and push the tag.
3. Desktop Build builds the four installers (Windows, macOS Apple Silicon,
   macOS Intel, Linux), checks that each is a real, non-empty file, and
   creates a **draft** release carrying them.
4. Its last step reads the release's assets back from GitHub and fails if a
   platform has no installer (`verify_artifacts.py --release-assets`).
5. Review the notes and publish the draft.

## A release that already exists without installers

Run Desktop Build by hand (`workflow_dispatch`) with `release_tag` set to the
tag. The installers are built from that tag, attached to the existing release
without touching its notes or status, then verified the same way:

```bash
gh workflow run "Desktop Build" --ref feat/x.y.z -f release_tag=0.14.1
```

## Update banner

The desktop app sets `KRONN_DESKTOP_APP=1`. The banner then only offers a
release that has an installer for the user's platform. Docker and source
installs update from the tag, so the banner offers them every release.
