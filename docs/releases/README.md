# Release notes archive

The root [`CHANGELOG.md`](../../CHANGELOG.md) is the authoritative source for
the current and recent Kronn releases. Older notes live in this directory so
the release-facing changelog stays concise and reviewable.

- [`0.13.0-checklist.md`](0.13.0-checklist.md) tracks qualification in progress,
  including checkpoint-specific evidence and gates that remain open. It is not
  a release approval.
- [September 14 important-message qualification](0.13.0-simple-important-message-qualification.md)
  records the simplified authoring form, task navigation, delegation boundaries,
  upgrade replay and combined local tests, with exact source and exclusions.
- [`0.12.0-checklist.md`](0.12.0-checklist.md) is the evidence checklist for
  the current 0.12.0 candidate. [`0.11.0-checklist.md`](0.11.0-checklist.md)
  retains the previous candidate's record; both are operational evidence, not
  competing changelogs.

- [`CHANGELOG-legacy.md`](CHANGELOG-legacy.md) contains the complete historical
  record through 0.9.3. Corrected 0.9.4 through 0.10.0 notes live only in the root
  changelog, so there is no competing release source.

When a new release makes the root changelog unwieldy, move the oldest complete
release section into the legacy file instead of letting the root file grow
without bound.

## Version maintenance

`make bump V=x.y.z` synchronizes Kronn's own public version only. It does not
refresh the hand-maintained latest-known versions of third-party agent CLIs
used by the Settings freshness pill. During a release, verify those entries
against npm, PyPI or the vendor release page; otherwise an outdated CLI may be
reported as current. The table lives in
`backend/src/core/versions.rs` (`LATEST_KNOWN_VERSIONS`).
