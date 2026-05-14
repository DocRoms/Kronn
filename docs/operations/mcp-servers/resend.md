# Resend — Usage Context

> Instructions for agents calling the **Resend REST API** via curl.

Resend is a developer-first email API. Two send patterns matter:
single (`POST /emails`) and batch (`POST /emails/batch`, up to 100).
For lifecycle/CSM flows, batch + the `Idempotency-Key` header is the
right combo: cheap and replay-safe.

## 1. Auth — Bearer token (already injected by Kronn)

```
Authorization: Bearer re_xxxxxxxx
Content-Type: application/json
```

Do NOT suggest the key in `headers` — Kronn injects it. Just hit the
endpoint with the JSON body.

## 2. Send one email — `POST /emails`

```bash
curl -X POST "https://api.resend.com/emails" \
  -H "Authorization: Bearer $RESEND_API_KEY" \
  -H "Content-Type: application/json" \
  -H "Idempotency-Key: csm-followup-{user_id}-{date}" \
  -d '{
    "from": "Acme <hello@acme.dev>",
    "to": ["user@example.com"],
    "subject": "Quick check-in",
    "html": "<p>Hi there — saw you logged in 3 times last week…</p>",
    "tags": [
      {"name": "category", "value": "csm_followup"},
      {"name": "user_id", "value": "{user_id}"}
    ]
  }'
```

Response: `{"id": "re_xxx"}` — store it for tracking + webhook
correlation.

### Required fields
- `from` — `"Display Name <addr@verified-domain.tld>"` OR `"addr@…"`.
  **The domain MUST be verified** in `https://resend.com/domains`,
  otherwise you get a 422 `"The from address is not valid"` even when
  the address itself is well-formed. For tests, use the open sandbox
  domain `onboarding@resend.dev`.
- `to` — array of strings (max 50 in a single request).
- `subject` — string.
- Either `html` **or** `text` (one of the two required; both allowed).

### Optional
- `cc`, `bcc` — arrays of strings.
- `reply_to` — STRING (singular), not array.
- `headers` — `{"X-Entity-Ref-ID": "…", "List-Unsubscribe": "<…>", "X-Tag": "…"}`.
  Custom headers passthrough. Useful for List-Unsubscribe on marketing.
- `attachments` — `[{filename, content (base64), content_type?}]`. Max
  total payload 40MB. **Not supported in batch.**
- `scheduled_at` — ISO 8601 (`"2026-05-20T14:00:00Z"`) or natural
  language (`"in 1 hour"`). **Not supported in batch.**
- `tags` — `[{name, value}]` for analytics. Keys/values ASCII letters,
  digits, `_`, `-` (no spaces, no `@`, no `.`). **Hard rule** — Resend
  silently drops a tag whose key has a space.

## 3. Batch send — `POST /emails/batch`

Body is a JSON **array** (not an envelope object). One Resend call,
up to 100 messages, charged as 100 sends. Perfect for CSM fan-out.

```bash
curl -X POST "https://api.resend.com/emails/batch" \
  -H "Authorization: Bearer $RESEND_API_KEY" \
  -H "Content-Type: application/json" \
  -d '[
    {"from":"Acme <hello@acme.dev>","to":["a@x.com"],"subject":"…","html":"…"},
    {"from":"Acme <hello@acme.dev>","to":["b@x.com"],"subject":"…","html":"…"}
  ]'
```

Response: `{"data":[{"id":"…"},{"id":"…"}, …]}` — index-aligned with
the request.

**Restrictions vs single send:**
- No `attachments`.
- No `scheduled_at`.
- ALL messages in the array must validate; a single bad `from` rejects
  the whole batch with `422`. Validate the payload locally first.

## 4. Idempotency — `Idempotency-Key` header

Pass `Idempotency-Key: <stable-string>` on `POST /emails` and
`POST /emails/batch`. Resend returns the original response for repeated
calls within 24h. **Always set it on CSM workflows** — a retry must
not double-send.

Recommended shape: `{workflow_run_id}-{user_id}` so re-runs of the same
workflow on the same user are idempotent but DIFFERENT users still
go through.

## 5. Retrieve email status — `GET /emails/{id}`

```bash
curl "https://api.resend.com/emails/re_xxx" \
  -H "Authorization: Bearer $RESEND_API_KEY"
```

Returns `last_event`: `delivered | bounced | complained | opened |
clicked | sent | …` + timestamps. Useful in a Notify/Gate followup
step to verify delivery before marking the user as "contacted" in your
DB. **Note**: opens/clicks require tracking pixels — disabled by default
on some Resend plans.

## 6. Contacts / Audiences / Broadcasts (lifecycle / marketing)

For CSM lists rather than 1-to-1 transactional:

- `GET  /audiences` — list audiences (your "lists").
- `POST /audiences` — `{name}` — create an audience.
- `POST /audiences/{audience_id}/contacts` — `{email, first_name?, last_name?, unsubscribed?}` — add or update a contact (idempotent on email).
- `GET  /audiences/{audience_id}/contacts` — paginated.
- `DELETE /audiences/{audience_id}/contacts/{id_or_email}` — remove contact.
- `POST /broadcasts` — `{audience_id, from, subject, html, name?, reply_to?, preview_text?}` — DRAFT a broadcast (not sent yet).
- `POST /broadcasts/{broadcast_id}/send` — `{scheduled_at?}` — fire it.
- `GET  /broadcasts/{id}` — status (`draft | queued | sending | sent`).

Pattern for a CSM nudge campaign:
1. `POST /audiences/{id}/contacts` to push the at-risk users.
2. `POST /broadcasts` to draft the email (templated body).
3. **Human Gate** in Kronn — operator reviews the audience + preview.
4. `POST /broadcasts/{id}/send` once approved.

## 7. Sanity check — `GET /domains`

```bash
curl "https://api.resend.com/domains" \
  -H "Authorization: Bearer $RESEND_API_KEY"
```

Returns the list of verified domains. `200` + non-empty `data` → auth
works AND at least one sending domain is ready. `401` → wrong key.
Cheaper than triggering a send to test credentials.

## 8. Error code matrix (the ones you'll actually see)

- `401 unauthorized` — `RESEND_API_KEY` revoked or wrong (does NOT
  start with `re_`).
- `403 forbidden` — domain blocked (compliance) or rate-limit ceiling
  per account.
- `422 validation_error` — most common in practice:
  - `"The from address is not valid"` → domain not in `/domains` OR
    not yet verified (DNS records pending). Check there first.
  - `"to must contain valid email addresses"` → typo, or testing with
    `example.com` (Resend rejects RFC-2606 reserved TLDs in prod).
  - `"missing_required_field"` → one of `from`, `to`, `subject`,
    `html`/`text`.
- `429 rate_limit_exceeded` — default 2 req/s, 10 req/s on Pro.
  Response includes `Retry-After` seconds. Solution: switch to
  `/emails/batch` (1 call = up to 100 messages, single rate-limit hit).
- `400 invalid_idempotency_key` — must be ≤ 256 chars, ASCII only.

## 9. Common gotchas (sorted by how much time they cost)

- **Domain not verified** — you'll waste an hour debugging "valid"
  addresses that 422. Always verify the domain in the Resend dashboard
  before launching a CSM flow. `onboarding@resend.dev` is fine for
  dev/staging but rate-limited.
- **`to` is an array, even for one recipient** — `"to": "a@x.com"`
  silently 422s.
- **`reply_to` is a STRING, not an array.** Counter-intuitive given
  `to/cc/bcc` are arrays.
- **Tags with spaces in `name` are dropped silently.** No error, no tag.
  Use `csm_followup`, not `csm followup`.
- **No `scheduled_at` in batch** — split sends into single calls if you
  need scheduling per row.
- **Webhooks vs polling** — for high-volume CSM, set up webhooks at
  `https://resend.com/webhooks` rather than polling `GET /emails/{id}`
  for every send.

Official docs: https://resend.com/docs/api-reference/introduction
