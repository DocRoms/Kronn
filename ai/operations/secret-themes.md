# Secret unlocks (themes + profiles, bundled)

## What this is

Easter-egg / early-access mechanism. A user types a code, the backend
returns an array of `{ kind, name }` unlocks — a single code can unlock
a theme, a profile, several themes, or a bundle (e.g. `kronnBatman`
unlocks the Batman profile AND the Gotham theme in one shot). Same
code works on every self-hosted Kronn after update — share codes with
testers out-of-band.

## How it works

- **Backend** — `POST /api/themes/unlock` in `backend/src/api/themes.rs`
  hashes the posted code with plain SHA-256 and returns ALL matching
  entries from:
    1. `BUILT_IN_UNLOCK_HASHES` — `(kind, name, sha256_hex)` triples
       committed to the repo, shipped with every release. A code can
       appear on multiple rows → bundles. Kinds: `"theme"` | `"profile"`.
    2. `config.secret_themes` — plaintext theme overrides in
       `~/.config/kronn/config.toml`. Theme-only path for quick local
       testing; profile unlocks go through built-ins.
  Response: `{ unlocks: [{kind, name}, ...] }`. Generic `invalid code`
  error on miss.
- **Persistence** — theme unlocks are client-side in
  `localStorage['kronn:unlockedThemes']`. Profile unlocks are
  server-side in `config.unlocked_profiles: Vec<String>` (saved via
  `config::save` on unlock). Secret profiles whose id is not in that
  list are filtered out of `GET /api/profiles` and 404 on
  `GET /api/profiles/:id`.
- **Frontend** — `ThemeContext` gates theme usage on the unlocked
  list, persists additions, and dispatches `kronn:profiles-changed`
  when a profile unlock lands so every consumer (`ProfilesSection`,
  `NewDiscussionForm`, `DiscussionsPage`, `WorkflowWizard`) refetches
  live. The picker in Settings > Appearance shows unlocked themes; a
  Konami-gated "Secret Code" input submits the code.
- **CSS** — each theme has its own `:root[data-theme="<name>"]` block
  in `frontend/src/styles/tokens.css`.
- **Secret profiles** — declared in `backend/src/core/profiles.rs`
  via `SECRET_PROFILE_IDS`. Source lives in
  `backend/src/profiles/<id>.md` like any other built-in, the only
  difference is visibility gating.

## Adding / rotating a code

1. Pick a code (≥12 chars, dictionary resistance comes from length).
2. Generate its hash:
   ```sh
   echo -n 'YourCodeHere' | sha256sum | cut -d' ' -f1
   ```
3. Paste `("<kind>", "<name>", "<the-hex>")` into
   `BUILT_IN_UNLOCK_HASHES` in `backend/src/api/themes.rs`. Repeat on
   multiple rows with the SAME hash to build a bundle (one code → many
   unlocks).
4. Save the code in a password manager — nothing recovers it from the
   hash. Rotating = new code, new hash, replace the line(s), release.

## Adding a brand-new secret theme

1. `frontend/src/styles/tokens.css` — new
   `:root[data-theme="<name>"] { ... }` block (full palette override).
2. `frontend/src/lib/ThemeContext.tsx` — add `<name>` to the `ThemeMode`
   union AND to the `SECRET_THEMES` set, plus the `isValidTheme` guard.
3. `frontend/src/pages/SettingsPage.tsx` — label + icon in the
   `secretMeta` object of the picker.
4. `frontend/src/lib/i18n.ts` — `config.theme<Name>` for FR/EN/ES.
5. Add `("theme", "<name>", "<hash>")` to `BUILT_IN_UNLOCK_HASHES`.

## Adding a brand-new secret profile

1. `backend/src/profiles/<id>.md` — markdown with YAML frontmatter,
   same shape as any built-in profile (name, persona_name, role,
   avatar, color, category, body).
2. `backend/src/core/profiles.rs`:
   - Add a `BuiltinProfile { id: "<id>", content: include_str!(...) }`
     row to `BUILTIN_PROFILES`.
   - Add `"<id>"` to `SECRET_PROFILE_IDS`.
3. `BUILT_IN_UNLOCK_HASHES` in `api/themes.rs` — add
   `("profile", "<id>", "<hash>")`. Same hash on a theme row makes
   a bundle.
4. Optional: i18n string for a custom toast (see `batmanRecruited`
   pattern).

## Security caveats

- **Threat model**: easter eggs / trusted-tester access. Not designed
  to stop a determined attacker.
- Hash is unsalted — do not use common words as codes (trivially
  dictionary-attacked). Random/passphrase ≥12 chars is enough.
- A curious user can set `document.documentElement.setAttribute('data-theme', 'matrix')`
  in devtools and preview the palette without the code. They'll see
  colors but nothing gated. Real features must be gated server-side
  (signed token), not via `data-theme`.
- Error message is intentionally generic ("invalid code") so attackers
  can't enumerate configured themes via 404 vs. 200 variance.
- `localStorage['kronn:unlockedThemes']` tampering is filtered at mount
  (entries not in `SECRET_THEMES` are ignored) — no free upgrade.
