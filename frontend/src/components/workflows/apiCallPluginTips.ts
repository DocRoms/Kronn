// Per-plugin lore injected into the AI helper's system prompt.
//
// Why a static registry vs metadata on the plugin spec? The lore here is
// *debugging* knowledge ("404 on /live/* = host not in account"), not
// catalog metadata. It changes with vendor quirks more often than the
// endpoint shape, and we don't want to mix it into the typed `ApiSpec`
// that drives schema validation. A per-slug map is the cheapest source
// of truth and the easiest to grow as users hit new pitfalls.
//
// All entries are in French because the helper's UX is French. If we
// later add EN/ES we can switch to a `Record<lang, Record<slug, …>>`.

export interface PluginTips {
  /** Free-form prose injected as a `### TIPS PLUGIN` section. Keep each
   *  entry to a handful of bullets — agents do worse with walls of text
   *  than with terse cheat-sheets. */
  body: string;
  /** Human-readable docs URL the agent can point to when stumped. */
  docsUrl?: string;
}

export const PLUGIN_TIPS: Record<string, PluginTips> = {
  chartbeat: {
    body: `- Auth : query param \`apikey\` (déjà injecté par Kronn — ne le suggère JAMAIS).
- \`host\` (obligatoire sur la plupart des endpoints) doit matcher EXACTEMENT le domaine inscrit
  dans Settings → Sites du dashboard Chartbeat. Pas de \`fr.\` ou \`www.\` rajouté arbitrairement,
  pas de http(s) en préfixe, pas de slash final.
- 404 sur \`/live/*\` = host mal écrit OU non rattaché à la clé OU pas de trafic live à T.
  Recommander de tester un endpoint historique (ex \`/historical/traffic/series\`) avant de
  conclure que la clé est cassée — un succès historique prouve l'auth, isole le souci côté \`host\`.
- 403 = clé OK mais host pas autorisé pour cette clé.
- Endpoints retournent souvent un objet flat (\`{pages: [...]}\`) — \`extract: $.pages[*].path\` pour
  fan-out dans un BatchQuickPrompt.`,
    docsUrl: 'https://chartbeat.com/docs/api/explore/',
  },

  'mcp-github': {
    body: `- Auth : Bearer (token déjà injecté par Kronn — JAMAIS le suggérer dans \`headers\`).
- Les paths contiennent des placeholders \`{owner}\`, \`{repo}\`, \`{issue_number}\`, etc. — l'utilisateur DOIT
  les remplacer par les valeurs réelles dans le champ "Endpoint" (ex \`/repos/anthropics/anthropic-sdk-python/issues\`).
  Si tu vois un 404 sur un chemin contenant \`{...}\`, c'est qu'un placeholder n'a pas été substitué.
- Les listes d'issues incluent les PRs (les PRs SONT des issues côté GitHub). Filtrer par présence/absence
  du champ \`pull_request\` dans l'item si on ne veut que les vraies issues.
- 401 = token absent / expiré / scope insuffisant. 403 = rate-limit (header \`X-RateLimit-Remaining\` à 0)
  OU SSO-required pour les orgs avec SAML — l'utilisateur doit autoriser le PAT pour l'org dans son profil.
- 422 = paramètre invalide (ex \`state=invalid\`).
- Pagination : header \`Link\` (\`<…?page=2>; rel="next"\`). Pour itérer simplement, ajouter \`per_page=100\` (max).
- Search : \`/search/issues\` capé à 1000 résultats, le \`q=\` doit être URL-encodé (ex \`q=is%3Aissue+is%3Aopen+repo%3Aowner%2Fname\`).
- Pour le 1er test, recommander \`/user\` (sanity check de l'auth, retourne le profil du user du PAT).`,
    docsUrl: 'https://docs.github.com/en/rest',
  },

  // Keyed on the registry id (`server.id`) — the AI helper looks up
  // tips via `tipsForSlug(server?.id)`, not by display name. The `jira`
  // alias below stays for back-compat with manual lookups.
  'mcp-atlassian': {
    body: `- Auth : Basic \`email:token\` (Cloud) ou Bearer PAT (Server/DC) — déjà injecté par Kronn.
- ⚠️ \`/rest/api/3/search\` est SUPPRIMÉ depuis avril 2025 (CHANGE-2046, 410 Gone).
  Utiliser \`/rest/api/3/search/jql\` à la place — pagination cursor (\`nextPageToken\`),
  pas \`startAt\`. Pour le compteur total : \`POST /rest/api/3/search/approximate-count\`
  (séparé pour rester rapide). Si tu vois un 410 sur /search → c'est ça.
- 401 = token expiré ; 403 = pas le scope ; 404 = projet/issue inexistant ; 400 = JQL malformée.
- Le paramètre s'appelle \`jql\`, pas \`q\`. Toujours URL-encoder les espaces dans la valeur.
- Pagination : v3 search/jql = \`nextPageToken\` (cursor, le lit dans la response et le
  repasse en query param sur la requête suivante). \`isLast: true\` = fin.
- Champs custom (\`customfield_10016\` etc.) — un mapping est dispo via \`GET /rest/api/3/field\`.
- ADF (Atlassian Document Format) sur les descriptions/comments — pour avoir l'HTML rendu,
  ajouter \`expand=renderedFields\` à la requête issue.`,
    docsUrl: 'https://developer.atlassian.com/cloud/jira/platform/rest/v3/intro/',
  },
  // Alias kept for any manual lookup that uses the display-name slug.
  jira: {
    body: `- Auth : Basic \`email:token\` (Cloud) ou Bearer PAT (Server/DC) — déjà injecté par Kronn.
- ⚠️ \`/rest/api/3/search\` est SUPPRIMÉ depuis avril 2025 — utiliser \`/rest/api/3/search/jql\`
  avec pagination cursor (\`nextPageToken\`), pas \`startAt\`.
- 401 = token expiré ; 403 = pas le scope ; 404 = projet/issue inexistant ; 400 = JQL malformée.
- Le paramètre s'appelle \`jql\`, pas \`q\`. Toujours URL-encoder les espaces dans la valeur.
- Champs custom (\`customfield_10016\` etc.) — mapping dispo via \`GET /rest/api/3/field\`.`,
    docsUrl: 'https://developer.atlassian.com/cloud/jira/platform/rest/v3/intro/',
  },

  cloudflare: {
    body: `- Auth : Bearer API Token (refuser Legacy Key X-Auth-Key + X-Auth-Email — déprécié).
- GraphQL Analytics : si \`datetime_gt < résolution du dataset\`, retour silencieux vide → toujours
  caler la fenêtre sur la résolution (1m, 5m, 1h selon dataset).
- Maximum 7j d'historique sur la plupart des datasets analytics.
- 401 = token mort, 403 = scope insuffisant (vérifier "Account / Zone Read" dans les permissions).`,
    docsUrl: 'https://developers.cloudflare.com/analytics/graphql-api/',
  },

  'adobe-analytics': {
    body: `- Auth : OAuth2 client_credentials, plus un header \`X-API-Key\` séparé (déjà injecté).
- \`rsid\` (Report Suite ID) est obligatoire et casse-sensitive.
- 400 sur /reports = combinaison metric/breakdown invalide → simplifier d'abord, raffiner ensuite.`,
    docsUrl: 'https://developer.adobe.com/analytics-apis/docs/2.0/',
  },

  'google-search': {
    body: `- Auth : API Key query param \`key\` (déjà injecté par Kronn).
- \`cx\` (Custom Search Engine ID) est obligatoire — paramètre user.
- Quota par défaut : 100 requêtes/jour gratuites. 429 = quota épuisé.`,
    docsUrl: 'https://developers.google.com/custom-search/v1/overview',
  },

  'google-search-console': {
    body: `- Auth : OAuth2 client_credentials, scope readonly suffisant pour la plupart des endpoints.
- \`siteUrl\` doit matcher EXACTEMENT la propriété GSC. Domaine = pas de slash. URL prefix = avec slash final.
- 403 = propriété pas vérifiée pour l'identité OAuth.`,
    docsUrl: 'https://developers.google.com/webmaster-tools/v1/searchanalytics/query',
  },
};

/** Lookup by plugin slug. Returns `null` when no lore is registered — the
 *  prompt will simply omit the tips section, which is fine: the API spec
 *  alone is enough for many CRUD-style plugins. */
export function tipsForSlug(slug: string | null | undefined): PluginTips | null {
  if (!slug) return null;
  return PLUGIN_TIPS[slug] ?? null;
}
