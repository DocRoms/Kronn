# Precision of the audit secret scan (KT-1404)

`core::secret_facts` scans the tracked files and hands the audit security step a list of paths. The prompt turns every path detected by content into a finding, so a false positive is an audit error. This page records the method and the aggregate result. The per-path table lives outside this repository, because it concerns private third-party repositories. **No value is written anywhere: paths and kinds only.**

## Method

`SecretFacts::detect` was run on the `git ls-files` listing of each repository, at the code of commit `ffdf66fe3`, with a throwaway ignored test (since removed) that printed path, kind, class, and for credential lines the key name, line number and value length, never the value.

Classes, as the prompt words them:

- **confirmed**: content matched a secret shape outside a test location. A mandatory finding.
- **fixture**: content matched inside a test or fixture location. "To verify", Medium at most.
- **candidate**: name only. "To check", never a confirmed secret.

## Aggregate

- 4 repositories measured: 2 audit A/B bench repositories, 1 matrix repository, and Kronn itself. About 4 300 tracked text files read.
- 6 confirmed paths (2 private keys, 1 webhook URL and 1 literal credential in one repository; 2 files with literal credentials in another), **0 false positive among the confirmed**. Liveness of the credentials was not checked, so true positives hold by shape.
- 1 candidate (a tracked `.env`-named file in a test location), worded "to check", not a mandatory finding.
- 2 fixtures, both in Kronn: `backend/src/api/projects/used_skills.rs` and `backend/src/core/docs_write_filter.rs`, matched only in their `#[cfg(test)]` part, so not mandatory findings.
- No free-text check was added (the scan is facts only), so there is no 754-document corpus FP count.
