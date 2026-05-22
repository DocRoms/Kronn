// 0.8.6 — defensive sanitize for agent-emitted QP-improver payloads.
//
// The backend's `CreateQuickPromptRequest` uses `#[serde(default)]` which
// kicks in on MISSING fields but NOT on explicit `null` values. Agents
// sometimes emit `description: null`, `variables: null`, `skill_ids: null`
// → axum's Json extractor returns 422 with a cryptic "invalid type: null,
// expected a string" error.
//
// We also normalise the `tier` enum (`economy | default | reasoning`)
// because agents have been observed to hallucinate other values
// (`standard`, `medium`, `balanced`, …) — fallback on `default`.
//
// Pure fn so it can be unit-tested without spinning up React /
// fetch / a backend.

/** The 3 valid tier values per `backend/src/models/quick.rs::ModelTier`. */
export const VALID_QP_TIERS = ['economy', 'default', 'reasoning'] as const;
export type ValidQpTier = typeof VALID_QP_TIERS[number];

/** Mutates the payload in-place to coerce known offenders into shapes
 *  the backend will accept. Also strips any `id` the agent emitted — the
 *  URL drives PUT identity, never the body.
 *
 *  Returns the payload reference for chaining. Logs a `console.warn` for
 *  each normalisation so power users can trace what was rewritten.
 *
 *  Idempotent: running this twice produces the same output as once. */
export function sanitizeQpImproverPayload(
  payload: Record<string, unknown>,
): Record<string, unknown> {
  // The URL drives identity ; any `id` in the body is at best ignored,
  // at worst confusing if it points at a different QP.
  delete payload.id;

  // Coerce explicit nulls back to backend-friendly defaults. These
  // fields are NOT `Option<>` on the backend struct — they're
  // `Vec<>` / `String` with `#[serde(default)]`, which fails on
  // `null` but passes on absent.
  const nullToDefault: Array<[string, unknown]> = [
    ['description', ''],
    ['variables', []],
    ['skill_ids', []],
    ['profile_ids', []],
    ['directive_ids', []],
  ];
  for (const [key, def] of nullToDefault) {
    if (payload[key] === null || payload[key] === undefined) {
      payload[key] = def;
    }
  }

  // `icon`, `agent`, `project_id` are `Option<>` server-side — `null`
  // is fine. Leave them alone.

  // Normalise `tier`. Agents hallucinate sometimes. Lowercase the
  // valid values for case-insensitive match.
  if (payload.tier !== null && payload.tier !== undefined) {
    const raw = String(payload.tier).toLowerCase();
    if ((VALID_QP_TIERS as readonly string[]).includes(raw)) {
      payload.tier = raw;
    } else {
      console.warn(
        `QP deploy: agent emitted unknown tier "${payload.tier}", normalising to "default"`,
      );
      payload.tier = 'default';
    }
  }

  return payload;
}
