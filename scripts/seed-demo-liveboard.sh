#!/usr/bin/env bash
# Third seeding pass — the "real data, end to end" showcase.
#
# A workflow that collects two PUBLIC sources, hands them to an agent, and
# publishes the result into a Live Page you can then open and see refreshed.
# Nothing here is mocked: the weather is the weather, the headlines are the
# headlines, and a viewer can check both against the open web.
#
#   Open-Meteo   api.open-meteo.com   no key, no signup, no attribution
#   Euronews FR  fr.euronews.com/rss  public RSS, 50 items
#
# Chosen precisely because they need no credential. A demo that requires a
# secret is a demo nobody else can reproduce — and a sandbox that holds one is
# a sandbox that can leak it.
#
# Usage:  ./scripts/seed-demo-liveboard.sh      (after seed-demo-showcase.sh)
# Env:    KRONN_SANDBOX_PORT (default 3145)

set -euo pipefail

PORT="${KRONN_SANDBOX_PORT:-3145}"
API="http://localhost:${PORT}/api"

[ "$PORT" = "3140" ] && { echo "✗ 3140 is the real instance."; exit 1; }
curl -fsS "$API/setup/status" >/dev/null 2>&1 || { echo "✗ No sandbox on $PORT."; exit 1; }

post() {  # post <path> <json> <label>  -> response on stdout, tick on stderr
  local resp
  resp=$(curl -fsS -X POST -H "Content-Type: application/json" -d "$2" "$API$1" 2>&1 || true)
  case "$resp" in
    *'"success":true'*) echo "  ✓ $3" >&2; printf '%s' "$resp" ;;
    *) echo "  ✗ $3 — $(printf '%s' "$resp" | head -c 180)" >&2; printf '' ;;
  esac
}

field() { printf '%s' "$1" | grep -o "\"$2\":\"[^\"]*\"" | head -1 | cut -d'"' -f4; }

echo "▸ Sources publiques…"

# ── Two custom API plugins ─────────────────────────────────────────────
# `api-custom` materializes a fresh server from `custom_spec`. The declared
# endpoints are not decoration: the executor's allowlist refuses any call to a
# path that was never declared, so a plugin without them is inert.
METEO=$(post /mcps/configs '{
  "server_id":"api-custom","label":"Open-Meteo","env":{},"is_global":true,"project_ids":[],
  "custom_spec":{
    "name":"Open-Meteo",
    "base_url":"https://api.open-meteo.com",
    "description":"Prévisions météo ouvertes, sans clé ni inscription.",
    "docs_url":"https://open-meteo.com/en/docs",
    "endpoints":[{"path":"/v1/forecast","method":"GET","description":"Conditions courantes et prévisions pour une latitude/longitude."}]
  }
}' "Plugin Open-Meteo")

# Euronews publishes RSS, which is XML — and a Quick API decodes JSON only
# ("Response JSON parse failed (200)"). Rather than pretend otherwise, the
# source is read through a public keyless RSS-to-JSON bridge. Same articles,
# same publisher, a shape Kronn can actually consume.
RSS=$(post /mcps/configs '{
  "server_id":"api-custom","label":"Euronews FR (RSS→JSON)","env":{},"is_global":true,"project_ids":[],
  "custom_spec":{
    "name":"Euronews FR (RSS→JSON)",
    "base_url":"https://api.rss2json.com",
    "description":"Flux RSS public d'"'"'Euronews en français, converti en JSON par un pont sans clé.",
    "docs_url":"https://rss2json.com/docs",
    "endpoints":[{"path":"/v1/api.json","method":"GET","description":"Convertit un flux RSS en JSON. Paramètre rss_url."}]
  }
}' "Plugin Euronews FR")

METEO_SLUG=$(field "$METEO" server_id); METEO_CFG=$(field "$METEO" id)
RSS_SLUG=$(field "$RSS" server_id);     RSS_CFG=$(field "$RSS" id)
echo "    météo: ${METEO_SLUG:-?} / ${METEO_CFG:-?}"
echo "    rss  : ${RSS_SLUG:-?} / ${RSS_CFG:-?}"

# ── One Quick API per source ───────────────────────────────────────────
# Saved rather than hand-built: a Quick API is replayable and audited, which is
# the whole reason Kronn prefers it to a raw call.
echo
echo "▸ Quick APIs…"
[ -n "$METEO_CFG" ] && post /quick-apis "{
  \"name\":\"Météo — Lyon maintenant\",\"icon\":\"🌤️\",
  \"description\":\"Température, vent et code météo pour Lyon, en direct.\",
  \"api_plugin_slug\":\"$METEO_SLUG\",\"api_config_id\":\"$METEO_CFG\",
  \"api_endpoint_path\":\"/v1/forecast\",\"api_method\":\"GET\",
  \"api_query\":{\"latitude\":\"45.76\",\"longitude\":\"4.84\",\"current\":\"temperature_2m,weather_code,wind_speed_10m\",\"timezone\":\"Europe/Paris\"},
  \"variables\":[]
}" "Météo — Lyon maintenant" >/dev/null

[ -n "$RSS_CFG" ] && post /quick-apis "{
  \"name\":\"Euronews — derniers articles\",\"icon\":\"📰\",
  \"description\":\"Les publications les plus récentes du flux RSS français.\",
  \"api_plugin_slug\":\"$RSS_SLUG\",\"api_config_id\":\"$RSS_CFG\",
  \"api_endpoint_path\":\"/v1/api.json\",\"api_method\":\"GET\",
  \"api_query\":{\"rss_url\":\"https://fr.euronews.com/rss\"},
  \"variables\":[]
}" "Euronews — derniers articles" >/dev/null

cat <<'EOF'

✓ Sources seeded  (page HTML: scripts/demo-liveboard-page.html)

Les deux Quick APIs interrogent des services publics réels, sans aucune clé.
Prochaine étape : le workflow qui les collecte, les fait relire par un agent
et publie dans une Live Page.
EOF
