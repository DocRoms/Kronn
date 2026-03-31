---
name: gdpr
description: Use when handling personal data, user accounts, analytics, consent flows, or third-party data sharing. Ensures GDPR/CCPA compliance with data minimization, consent management, and breach readiness.
license: AGPL-3.0
category: business
icon: 🛡️
builtin: true
---

## Procedure

1. **Audit data flow**: identify every field containing PII. For each, document lawful basis (consent, contract, legitimate interest).
2. **Minimize collection**: remove fields you don't strictly need. If you don't need birth date, don't ask.
3. **Implement consent**: must be explicit, informed, granular. Provide equal-effort withdrawal. No pre-checked boxes.
4. **Secure storage**: encrypt PII at rest. Define retention periods per data type. Automate deletion after expiry.
5. **Enable data subject rights**: access, rectification, erasure, portability, objection. Each must have a working endpoint or flow.
6. **Scrub logs**: verify no PII appears in application logs, error traces, or analytics.

## Gotchas

- Pseudonymized data is still personal data under GDPR if re-identification is possible. `user_id=42` in logs is fine; `user_id=42` next to `email=...` in the same DB is not pseudonymization.
- IP addresses are PII. Anonymize in analytics (truncate last octet minimum).
- "Legitimate interest" is not a free pass — it requires a documented balancing test.
- Consent collected before explaining the purpose is invalid. The consent UI must appear AFTER the explanation.
- 72-hour breach notification starts from AWARENESS, not from discovery of impact.

## Validation

Grep logs/output for email patterns, IP addresses, phone numbers. Check DB schema for missing retention/TTL columns. Verify consent withdrawal flow works end-to-end.

✓ Log: `user_id=42 action=login` — pseudonymized, no PII
✗ Log: `email=user@test.com ip=1.2.3.4 action=login` — PII in plaintext logs
