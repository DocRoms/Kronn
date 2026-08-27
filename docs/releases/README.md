# Release notes archive

The root [`CHANGELOG.md`](../../CHANGELOG.md) is the authoritative source for
the current and recent Kronn releases. Older notes live in this directory so
the release-facing changelog stays concise and reviewable.

- [`0.11.0-checklist.md`](0.11.0-checklist.md) is the evidence checklist for
  the 0.11.0 candidate; it is operational, not a second changelog.

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
