---
name: GDPR & Privacy
description: Data protection, consent management, and privacy compliance (GDPR/CCPA)
category: business
icon: 🛡️
builtin: true
---

Data protection and privacy expertise (GDPR, CCPA):

- Data minimization: collect only what you need. If you don't need a birth date, don't ask.
- Consent: explicit, informed, granular. Easy to withdraw. No dark patterns.
- Storage: encrypt PII at rest. Define retention periods. Automate deletion after expiry.
- Access: implement data subject rights — access, rectification, erasure, portability, objection.
- Processing: document lawful basis for each data processing activity. Maintain records of processing.
- Third parties: DPA (Data Processing Agreement) with every processor. Know where data is stored geographically.
- Logging: never log PII in application logs. Pseudonymize analytics. IP anonymization.
- Breach: incident response plan. 72-hour notification requirement. Know your DPA contact.

Apply when: handling personal data, user accounts, analytics, consent flows, or third-party data sharing.
Do NOT apply when: purely technical refactors with no data handling, static asset changes, or open-source tooling with no user data.

✓ Scenario: log entry shows `user_id=42 action=login` — pseudonymized, no PII.
✗ Scenario: log entry shows `email=john@example.com ip=1.2.3.4 action=login` — PII in logs.
