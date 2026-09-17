#!/usr/bin/env bash
# Second seeding pass for the screenshot sandbox — the 0.13.0 surfaces.
#
# `seed-demo-fixtures.sh` builds the instance and fills it with projects,
# Quick Prompts and one workflow: enough for a dashboard, not enough for the
# screens that sell the release. This adds what those need — named external
# connections with their media slots, a multi-agent room with a real
# transcript, a media generation thread, and a Quick Prompt whose comparison
# has something to compare.
#
# Everything here is written over plain HTTP against the sandbox. NO agent is
# launched and no provider is called: a transcript is appended through
# `/api/disc/append`, exactly as a joined CLI peer would. That matters for
# three reasons — it costs nothing, it is deterministic, and it can be
# regenerated identically after a UI change, which a real multi-agent run
# never could.
#
# The credentials seeded here are obvious fakes. A screenshot sandbox that
# needs a real key is a screenshot sandbox that can leak one.
#
# Usage:  ./scripts/seed-demo-showcase.sh          (after seed-demo-fixtures.sh)
# Env:    KRONN_SANDBOX_PORT (default 3145)

set -euo pipefail

PORT="${KRONN_SANDBOX_PORT:-3145}"
API="http://localhost:${PORT}/api"

command -v curl >/dev/null || { echo "✗ curl is required"; exit 1; }
curl -fsS "$API/setup/status" >/dev/null 2>&1 || {
  echo "✗ No sandbox on port $PORT. Run ./scripts/seed-demo-fixtures.sh first."
  exit 1
}

# Refuse to run against the maintainer's own instance. The default is 3140 and
# the whole point of this file is that it writes fabricated data.
if [ "$PORT" = "3140" ]; then
  echo "✗ 3140 is the real instance. This script writes demo data; pick the sandbox port."
  exit 1
fi

# A real credential is opt-in and named, never picked up from the environment
# by accident: capturing a media screenshot needs one generation to actually
# run, and nothing else here does. Unset, every connection gets an obvious
# fake and the sandbox can be published without a second thought.
OPENROUTER_KEY="sk-demo-not-a-real-key"
if [ -n "${KRONN_DEMO_OPENROUTER_KEY_FILE:-}" ]; then
  if [ -r "$KRONN_DEMO_OPENROUTER_KEY_FILE" ]; then
    OPENROUTER_KEY=$(tr -d '\r\n' < "$KRONN_DEMO_OPENROUTER_KEY_FILE")
    echo "▸ OpenRouter: real credential (from a named file, never echoed)"
  else
    echo "✗ KRONN_DEMO_OPENROUTER_KEY_FILE is set but unreadable"; exit 1
  fi
fi

echo "▸ Showcase seeding on port ${PORT}…"

post() {  # post <path> <json> <label>
  local path="$1" body="$2" label="$3" resp
  resp=$(curl -fsS -X POST -H "Content-Type: application/json" -d "$body" "$API$path" 2>&1 || true)
  case "$resp" in
    # The tick goes to stderr: callers capture stdout to read an id out of the
    # response, and a success line mixed into it would be captured too.
    *'"success":true'*) echo "  ✓ $label" >&2; printf '%s' "$resp" ;;
    *) echo "  ✗ $label — $(printf '%s' "$resp" | head -c 200)" >&2; printf '' ;;
  esac
}

# Reads one string field out of a JSON response without jq, keeping this
# script's "pure bash + curl" contract.
#
# `grep -o` rather than a `sed` substitution: a leading `.*` is greedy, so the
# substitution returned the LAST match of the key. A create response carries
# several ids — the object's, and the first message's — so that silently
# handed back a message id and every append went to a discussion that did not
# exist. The first match is the one that belongs to the object.
field() {  # field <json> <key>
  printf '%s' "$1" | grep -o "\"$2\":\"[^\"]*\"" | head -1 | cut -d'"' -f4
}

# ── Named external connections ─────────────────────────────────────────
# Three presets, because the screen worth capturing is the one showing that a
# connection is an agent like any other: its own alias, its own three tiers,
# and — for the two that serve media — its image and video slots.
echo
echo "▸ External API connections…"

OPENROUTER=$(post /external-api/connections '{
  "display_name":"OpenRouter",
  "mention_alias":"openrouter",
  "endpoint":"https://openrouter.ai/api",
  "origin_preset":"open_router",
  "economy_model":"qwen/qwen3.8-flash",
  "default_model":"z-ai/glm-5.3",
  "reasoning_model":"qwen/qwen3.8-max-0902",
  "image_model":"google/gemini-3.1-flash-image",
  "video_model":"bytedance/seedance-1-5-pro",
  "api_key":"'"$OPENROUTER_KEY"'"
}' "OpenRouter — 3 tiers + image + video")

# These two already exist: Kronn creates a legacy connection per built-in HTTP
# provider at first boot, so their aliases are taken. Fill them in rather than
# duplicate them — an empty tier list is exactly what the screenshot must not
# show.
put() {  # put <path> <json> <label>
  local resp
  resp=$(curl -fsS -X PUT -H "Content-Type: application/json" -d "$2" "$API$1" 2>&1 || true)
  case "$resp" in
    *'"success":true'*) echo "  ✓ $3" >&2 ;;
    *) echo "  ✗ $3 — $(printf '%s' "$resp" | head -c 160)" >&2 ;;
  esac
}

put /external-api/connections/external-api-litellm '{
  "display_name":"LiteLLM",
  "mention_alias":"litellm",
  "endpoint":"https://litellm.demo.internal",
  "origin_preset":"lite_llm",
  "economy_model":"gpt-4.1-mini",
  "default_model":"claude-sonnet-4-6",
  "reasoning_model":"gpt-5.1",
  "api_key":"sk-demo-not-a-real-key"
}' "LiteLLM — routed tiers"

put /external-api/connections/external-api-nvidia '{
  "display_name":"NVIDIA",
  "mention_alias":"nvidia",
  "endpoint":"https://integrate.api.nvidia.com",
  "origin_preset":"nvidia",
  "default_model":"meta/llama-3.3-70b-instruct",
  "api_key":"nvapi-demo-not-a-real-key"
}' "NVIDIA — single tier"

CONNECTION_ID=$(field "$OPENROUTER" id)

echo
echo "▸ Discussions…"

# ── A multi-agent room ─────────────────────────────────────────────────
# The screen this exists for: three different agents answering the same
# question in one thread, each labelled with who it is. Appended rather than
# run, so the wording is stable across captures.
ROOM=$(post /discussions '{
  "project_id":null,
  "title":"Faut-il migrer le cache Redis vers Lambda ?",
  "agent":"ClaudeCode",
  "language":"fr",
  "initial_prompt":"@claude @codex @openrouter — un flush Redis lancé sur une instance frontale la bloque, l'\''ALB la sort du pool, et on part en cascade de 503. Trois avis : est-ce qu'\''on déplace le flush vers une Lambda, et qu'\''est-ce que ça casse ?"
}' "Room multi-agent")
ROOM_ID=$(field "$ROOM" id)

if [ -n "$ROOM_ID" ]; then
  append() {  # append <source_id> <agent_type> <content>
    local body
    body=$(printf '{"disc_id":"%s","messages":[{"source_msg_id":"%s","role":"Agent","content":%s,"agent_type":"%s"}]}' \
      "$ROOM_ID" "$1" "$3" "$2")
    # Checked on the payload, not on curl's exit code: the API answers HTTP 200
    # with `success:false` for a refused append, so an exit-code check reported
    # three replies that were never written.
    local resp
    resp=$(curl -fsS -X POST -H "Content-Type: application/json" -d "$body" \
      "$API/disc/append" 2>&1 || true)
    case "$resp" in
      *'"success":true'*) echo "  ✓ réponse $2" >&2 ;;
      *) echo "  ✗ réponse $2 — $(printf '%s' "$resp" | head -c 140)" >&2 ;;
    esac
  }

  append demo-1 ClaudeCode '"Oui, mais pas pour la raison qu'\''on croit.\n\nLe problème n'\''est pas le flush : c'\''est qu'\''il s'\''exécute **dans** le processus qui sert le trafic. `RedisFlushController` bloque le worker, l'\''ALB voit un health check qui ne répond plus, et sort l'\''instance du pool. Le 503 est une conséquence de l'\''éviction, pas du flush.\n\nDéplacer vers Lambda règle ça. Ce que ça casse : le flush devient asynchrone, donc il faut un signal de fin — sinon on purge et on continue à servir du cache périmé sans le savoir."'

  append demo-2 Codex '"D'\''accord sur le diagnostic, en désaccord sur la priorité.\n\nLa cascade a une deuxième moitié que la Lambda ne touche pas : quand les instances restantes absorbent le trafic, Apollo prend une vague de cache-miss et sature. Les 500 qu'\''on voit ensuite viennent de là, et ils durent plus longtemps que les 503.\n\nDonc la Lambda enlève le déclencheur, pas l'\''amplificateur. Si on ne fait que ça, le prochain incident sera plus court mais pas évité."'

  append demo-3 Custom '"Je regarde les chiffres plutôt que l'\''architecture.\n\nSur les 24 dernières heures : 82 754 erreurs 5xx au CDN, pic à 5 270 sur une heure, et une chute à 1 433 sur la dernière. La répartition horaire ne montre pas un incident unique — elle montre une charge de fond.\n\nÇa change l'\''arbitrage : si le flush était la cause principale, on verrait des pics isolés. Là il y a autre chose, et la Lambda ne le trouvera pas."'
fi

# ── A media generation thread ──────────────────────────────────────────
if [ -n "$CONNECTION_ID" ]; then
  MEDIA=$(post /discussions "{
    \"project_id\":null,
    \"title\":\"Visuel — tableau de bord des erreurs 5xx\",
    \"agent\":\"Custom\",
    \"connection_id\":\"$CONNECTION_ID\",
    \"language\":\"fr\",
    \"initial_prompt\":\"Génère une image conceptuelle : un tableau de bord d'analyse montrant un pic d'erreurs serveur, graphiques rouges, mode sombre, style analytique professionnel.\"
  }" "Room génération média")
  MEDIA_ID=$(field "$MEDIA" id)
  if [ -n "$MEDIA_ID" ]; then
    curl -fsS -X POST -H "Content-Type: application/json" -d "$(printf '{"disc_id":"%s","messages":[{"source_msg_id":"demo-media-1","role":"Agent","content":%s,"agent_type":"Custom"}]}' \
      "$MEDIA_ID" '"L'\''image est lancée sur `black-forest-labs/flux-1.1-pro`.\n\nLe modèle annonce 1024×1024, 1344×768 et 768×1344 ; j'\''ai pris le format large, qui correspond à un tableau de bord. Coût estimé avant envoi : 0,04 $.\n\nElle arrivera dans les fichiers de contexte de la discussion toute seule — pas besoin de sonder."')" \
      "$API/disc/append" >/dev/null 2>&1 && echo "  ✓ réponse média" || echo "  ✗ réponse média" >&2
  fi
fi

# ── A Quick Prompt worth comparing ─────────────────────────────────────
echo
echo "▸ Quick Prompt de comparaison…"
post /quick-prompts '{
  "name":"Comparer les modèles sur un diagnostic",
  "icon":"⚖️",
  "prompt_template":"Voici les symptômes d'\''un incident de production :\n\n{{symptomes}}\n\nDonne la cause la plus probable, ce qui l'\''infirmerait, et la première mesure à prendre. Sois bref.",
  "variables":[{"name":"symptomes","label":"Symptômes observés","placeholder":"503 en rafale après un déploiement…","required":true}],
  "agent":"Custom","connection_id":"'"$CONNECTION_ID"'","tier":"default","skill_ids":[],
  "description":"Le même diagnostic posé à plusieurs modèles, pour comparer les réponses côte à côte sur un run unique."
}' "Comparer les modèles sur un diagnostic" >/dev/null

cat <<EOF

✓ Showcase seeded

  Connexions : OpenRouter (3 tiers + image + vidéo), LiteLLM, NVIDIA
  Rooms      : une multi-agent (3 réponses), une génération média
  Quick Prompt : un prompt de comparaison

Aucun agent n'a été lancé, aucun fournisseur appelé, aucune clé réelle écrite.
EOF
