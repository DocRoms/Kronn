# Security Policy

## Supported Versions

Kronn is in active pre-1.0 development. Security fixes are applied to:

- The **latest minor release** (e.g. `0.8.x` while 0.8 is current).
- The `main` branch.

Older minor versions do not receive backported fixes. Upgrade to the
latest tag for security updates.

| Version | Supported          |
| ------- | ------------------ |
| 0.8.x   | :white_check_mark: |
| < 0.8   | :x:                |

## Reporting a Vulnerability

**Please do not open a public GitHub issue for security reports.** Public
issues are indexed by search engines and notify everyone watching the
repo before a fix is available.

Instead, use **GitHub's private security advisories**:

➡️ [Report a vulnerability](https://github.com/DocRoms/Kronn/security/advisories/new)

This route is private, read by the maintainers only, and lets us
coordinate a fix and a coordinated disclosure with you.

### What to include

A useful report contains:

- Affected version (output of `kronn --version` or the release tag).
- A minimal reproduction or steps to trigger the issue.
- The impact you observed (information disclosure, RCE, auth bypass,
  data loss, denial of service…).
- Your suggested severity (Low / Medium / High / Critical), if you have
  one.
- Whether the issue is already public anywhere.

### What you can expect

- **Acknowledgement** within 5 business days.
- **Initial assessment** within 10 business days, including a target fix
  window.
- **Fix + advisory** published together once the patch ships. The
  advisory will credit you (unless you ask to remain anonymous).

We aim to ship security fixes in the next patch release. For Critical
issues with a working exploit, we may ship an out-of-band patch.

## Scope

In scope:

- The Kronn backend binary (Rust / Axum).
- The Kronn frontend (React, served by the backend).
- The desktop bundles (Tauri / packaged builds).
- The default MCP servers and templates shipped with Kronn.

Out of scope:

- Third-party AI providers (Anthropic, OpenAI, Google…) — report to the
  vendor.
- Third-party MCP servers the user installs themselves.
- Self-hosted misconfigurations where a user deliberately exposes the
  backend port without authentication on a public network.
- Findings from automated scanners without a working proof of concept.

## Hardening notes (for self-hosting)

Kronn is designed to run locally or on a trusted network. If you expose
it beyond `localhost`:

- Always enable authentication (`auth_enabled`) and put it behind a
  reverse proxy with TLS.
- Treat the encrypted vault directory (`~/.kronn`) as a secret — back
  it up but don't store it on shared drives.
- Audit the MCP servers you install: an MCP server runs with the same
  filesystem access as the agent it serves.

These are not vulnerabilities in Kronn itself, but they're the most
common ways a Kronn deployment ends up exposed.
