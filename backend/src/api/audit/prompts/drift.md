DRIFT AUDIT — report checksum mismatches without rewriting documentation.

Read docs/checksums.json. For every (file, sha256) recorded there, recompute the sha and list the files where the recorded hash no longer matches. Do NOT rewrite any file — your output is a short bullet list of stale docs/ files only.