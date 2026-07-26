# Prise en main & architecture

> Comprendre le trajet d'une requête API dans Kronn

- **Niveau** : non précisé
- **Prérequis** : Aucun
- **Références** : —

<!-- Cours généré par le Mode Mentor (posture onboarding). Régénéré à chaque (re)génération du parcours. -->

## 1. Lancer Kronn en local : Docker vs mode natif

À la fin de ce chapitre, tu sauras démarrer Kronn sur ta machine et choisir le bon mode selon ton OS.

Kronn est "Docker-first" : tout le stack (backend + frontend + gateway) tourne dans des conteneurs, ce qui garantit que ça marche pareil chez tout le monde. Mais il y a une limite importante sur macOS : Docker Desktop ne peut pas exécuter les binaires macOS des agents CLI (Claude Code, Codex...) installés sur ton Mac, ni lire ton trousseau (Keychain) — ce sont des exécutables Darwin, incompatibles avec le conteneur Linux `[src: file: docs/AGENTS.md:100-102]`. C'est pourquoi Kronn propose deux portes d'entrée :

- `./kronn start` (ou `make start`) : lance tout en Docker, profil "fast" sans optimisation LTO pour compiler ~4x plus vite `[src: file: Makefile:159-177]`.
- `./kronn start-dev` : mode 100% natif (pas de Docker), équivalent à lancer `make dev-backend` (cargo watch, hot-reload Rust) et `make dev-frontend` (Vite) dans deux terminaux. C'est le mode recommandé sur macOS pour que les agents s'exécutent réellement sur l'hôte.

Avant de construire quoi que ce soit, `make start` génère un fichier `.env` qui auto-détecte ton OS, ton UID/GID, et les chemins de tes outils (cargo, npm...) — c'est la cible `.env:` du Makefile `[src: file: Makefile:30-127]`. Une fois lancé, l'interface est sur `http://localhost:3140`.

Idée maîtresse : le choix Docker vs natif n'est pas cosmétique — sur macOS il détermine si tes agents CLI peuvent réellement s'exécuter.

**Checkpoint — Quiz**

Sur macOS, quelle commande faut-il utiliser pour que les agents CLI (Claude Code, Codex...) tournent nativement sur l'hôte plutôt que dans le conteneur Docker ?

A. ./kronn start
B. ./kronn start-dev
C. docker compose up -d --build
D. make build

<details>
<summary>Voir le corrigé</summary>

**Réponse : B**

- **A.** Faux — './kronn start' lance le stack complet dans Docker ; sur macOS, Docker ne peut pas exécuter les binaires Claude/Codex de l'hôte ni lire le Keychain.
- **B.** Correct — './kronn start-dev' (= make dev-backend + make dev-frontend) tourne 100% en natif sur ta machine : les agents CLI s'exécutent directement, sans les limites de Docker sur macOS.
- **C.** Faux — c'est la commande brute que './kronn start' orchestre en interne (génération du .env, flags de profil) ; l'appeler seule saute ces étapes et reste de toute façon en Docker.
- **D.** Faux — 'make build' compile un binaire de prod natif, il ne lance rien et ne gère aucun mode dev.

</details>

## 2. Les 3 services Docker de Kronn

À la fin de ce chapitre, tu sauras nommer les 3 services Docker de Kronn et le rôle exact de chacun.

`docker-compose.yml` définit trois services distincts, chacun avec une seule responsabilité `[src: file: docker-compose.yml:1-236]` :

- **backend** : le serveur Rust/axum, écoute en interne sur le port 3140. C'est lui qui contient toute la logique métier et parle à SQLite.
- **frontend** : le build React (Vite) servi par nginx, port interne 80 (exposé sur l'hôte en 3141 pour un accès direct de debug).
- **gateway** : un nginx dédié, port interne 80, mappé sur l'hôte en 3140 par défaut (`${KRONN_BIND:-127.0.0.1}:3140:80`) — c'est le SEUL point d'entrée que l'utilisateur touche `[src: file: docker-compose.yml:225-228]`.

Pourquoi cette séparation plutôt qu'un unique serveur qui fait tout ? Le gateway route `/api/*` vers le backend et le reste vers le frontend `[src: file: .docker/nginx.conf]` — ainsi React ne parle JAMAIS directement à SQLite ou au processus Rust, il passe toujours par une frontière HTTP claire. Ça permet aussi au nginx du gateway d'ajouter des couches transverses sans toucher au code Rust : compression gzip, rate-limiting (30 req/s), headers de sécurité, et un timeout long (1800s) spécifiquement pour les routes de streaming SSE (discussions, audits).

Idée maîtresse : "backend", "frontend" et "gateway" ne sont pas des synonymes de "le serveur" — chacun a un rôle unique et remplaçable indépendamment des deux autres.

**Checkpoint — Quiz**

Quel service traite réellement le code métier d'une requête vers /api/projects ?

A. Le service frontend (nginx qui sert le build Vite)
B. Le service gateway (nginx)
C. Le service backend (axum/Rust)
D. SQLite directement, sans passer par un serveur HTTP

<details>
<summary>Voir le corrigé</summary>

**Réponse : C**

- **A.** Faux — le frontend ne fait que servir les fichiers statiques React ; il ne contient aucune logique /api/*.
- **B.** Faux — le gateway est un simple proxy : il route la requête vers le bon service mais ne l'exécute jamais lui-même.
- **C.** Correct — le backend axum/Rust est le seul service qui exécute les handlers, lit la configuration et parle à SQLite.
- **D.** Faux — SQLite est une base fichier locale sans serveur réseau ; c'est le backend qui ouvre la connexion via Database::with_conn().

</details>

## 3. Le trajet d'une requête, du navigateur au JSON

À la fin de ce chapitre, tu sauras décrire, étape par étape, ce qui se passe entre un clic dans le navigateur et la réponse JSON qui s'affiche.

Quand le frontend appelle `fetch('/api/projects/abc123')`, ce chemin est relatif : le navigateur le résout vers l'hôte courant, donc vers le gateway (`localhost:3140`). Le trajet complet, résumé dans `docs/architecture/overview.md` `[src: file: docs/architecture/overview.md:375-383]`, ressemble à ceci :

1. Le navigateur envoie la requête au gateway nginx.
2. nginx ajoute l'en-tête `X-Real-IP` (l'IP réelle du client) et fait un `proxy_pass` vers `http://backend:3140` `[src: file: .docker/nginx.conf]`.
3. axum reçoit la requête et traverse ses `layer()`s : CORS (`build_cors()`), puis `TraceLayer` (logs), puis — si l'authentification est activée — le middleware d'auth `[src: file: backend/src/lib.rs:914-919]`.
4. Le routeur construit dans `build_router()` matche la méthode HTTP + le chemin déclarés, et appelle le handler correspondant.
5. Le handler lit ou écrit l'état partagé (base SQLite, config...) et répond un JSON.

Pourquoi retenir cet ordre précis ? Parce que si un bug apparaît (401 inattendu, CORS bloqué, route introuvable), savoir à QUELLE étape chercher économise un temps monstre — un souci de CORS ne se débogue pas au même endroit qu'un souci de route mal enregistrée.

Idée maîtresse : une requête traverse toujours DEUX couches de routage avant d'atteindre le code métier — nginx (gateway) PUIS axum (backend) — chacune avec sa propre responsabilité.

**Checkpoint — Exercice**

Le frontend fait un fetch('/api/projects/abc123'). Décris, dans l'ordre, les étapes que traverse cette requête avant que le JSON n'arrive dans le navigateur.

<details>
<summary>Voir le corrigé</summary>

1) Le navigateur envoie la requête à localhost:3140 (le gateway nginx, seul point d'entrée). 2) nginx (gateway) reçoit sur /api/*, ajoute l'en-tête X-Real-IP, et fait un proxy_pass vers http://backend:3140. 3) axum reçoit la requête et applique ses layers dans l'ordre : CORS, puis TraceLayer, puis (si l'auth est activée) le middleware d'authentification. 4) Le routeur de build_router() matche la méthode + le chemin déclarés (ex. GET /api/projects/{id}) et appelle le handler correspondant. 5) Le handler lit/écrit l'état partagé (AppState → Database SQLite) et renvoie un Json<ApiResponse<T>> — c'est ce JSON que reçoit finalement le navigateur.

</details>

## 4. Le routeur axum : build_router()

À la fin de ce chapitre, tu sauras retrouver où une route est déclarée dans le code et éviter les deux pièges classiques d'axum 0.8.

Toutes les routes de Kronn vivent dans une seule fonction, `build_router_with_auth`, dans `backend/src/lib.rs:465`. Chaque ligne `.route("/chemin", get(handler))` mappe une méthode HTTP + un chemin vers une fonction Rust. Exemple réel tiré du code : `.route("/api/projects/{id}", get(api::projects::get))` `[src: file: backend/src/lib.rs:562]`.

Pourquoi `{id}` et pas `:id` ? Kronn utilise axum 0.8, qui a changé la syntaxe des paramètres de route par rapport à la version 0.7 (`:id`). Utiliser l'ancienne syntaxe aujourd'hui fait paniquer le routeur AU DÉMARRAGE du serveur — c'est un piège documenté explicitement dans le projet `[src: file: docs/AGENTS.md:131]`.

Deuxième piège : pour accepter GET et POST sur le même chemin, on chaîne les méthodes sur le MÊME `.route()` — `.route("/api/workflows", get(list).post(create))` — on n'appelle jamais deux fois `.route()` avec le même chemin, sinon panic également `[src: file: docs/AGENTS.md:129]`.

Idée maîtresse : `build_router()` est la carte complète de l'API Kronn — pour savoir "quel code traite telle requête", le réflexe est toujours de chercher le chemin dans `lib.rs` en premier.

**Checkpoint — Quiz**

En axum 0.8 (la version utilisée par Kronn), comment enregistre-t-on GET et POST sur le même chemin /api/workflows ?

A. .route("/api/workflows", get(list)).route("/api/workflows", post(create)) — deux appels .route() séparés
B. .route("/api/workflows", get(list).post(create)) — un seul .route() avec les méthodes chaînées
C. .route("/api/workflows/:id", get(list))
D. .get("/api/workflows", list).post("/api/workflows", create)

<details>
<summary>Voir le corrigé</summary>

**Réponse : B**

- **A.** Faux — enregistrer deux fois le même chemin dans deux .route() distincts fait paniquer axum au démarrage ; c'est un piège explicitement documenté dans ce projet.
- **B.** Correct — c'est exactement le pattern utilisé partout dans build_router(), par exemple pour /api/config/language ou /api/workflows.
- **C.** Faux — ':id' est la syntaxe axum 0.7 (obsolète) ; axum 0.8 attend '{id}', sinon la route panique à l'enregistrement.
- **D.** Faux — Router n'expose pas de méthodes .get()/.post() de ce type directement ; il faut toujours passer par .route(chemin, get(...).post(...)).

</details>

## 5. Le middleware d'authentification

À la fin de ce chapitre, tu sauras pourquoi une requête locale passe souvent sans token, et où ce choix est pris dans le code.

Après le routage, la requête retraverse un middleware appliqué globalement via `route_layer(middleware::from_fn_with_state(state, auth_middleware))` `[src: file: backend/src/lib.rs:917-919]`. Deux routes sont toujours laissées passer sans vérification, quoi qu'il arrive : `/api/health` (utilisé par le healthcheck Docker) et `/api/ws` (le WebSocket, qui s'authentifie autrement) `[src: file: backend/src/lib.rs:270-278]`.

Pour le reste, l'authentification est **opt-in** (`ServerConfig.auth_enabled`) : si elle est désactivée, ou qu'aucun token n'est configuré, tout passe. Si elle est activée, il faut soit un Bearer token valide, soit venir d'une IP considérée comme locale — loopback (`127.0.0.1`) ou passerelle Docker (`172.16.0.0/12`), mais PAS le LAN (`192.168.x.x`) ni Tailscale (`100.x.x.x`) `[src: file: backend/src/lib.rs:404-425]`.

Pourquoi ce choix ? Kronn est self-hosted : l'utilisateur sur sa propre machine ne doit pas se ré-authentifier sans arrêt, mais un pair distant sur le réseau, lui, doit prouver un token. C'est pour ça que sous Docker, une config fraîche démarre avec l'auth désactivée par défaut — sinon le premier lancement afficherait un 401 immédiat.

Idée maîtresse : l'authentification dans Kronn n'est pas "tout ou rien" — c'est une combinaison de "la fonctionnalité est-elle activée" ET "d'où vient la requête".

**Checkpoint — Quiz**

Sur une installation Docker par défaut, où 'auth_enabled' est désactivée, que se passe-t-il pour une requête normale (non destructive) venant de ta propre machine ?

A. Elle est bloquée, un Bearer token est toujours exigé
B. Elle passe sans vérification, car l'authentification est un mécanisme opt-in
C. Elle est redirigée automatiquement vers /api/health
D. Le serveur refuse de démarrer tant qu'aucun token n'est configuré

<details>
<summary>Voir le corrigé</summary>

**Réponse : B**

- **A.** Faux — 'auth_enabled' est justement le master switch : quand il est à false, le middleware d'auth laisse tout passer (sauf les endpoints destructifs comme DELETE).
- **B.** Correct — l'authentification est opt-in dans Kronn ; par défaut sous Docker elle est désactivée pour éviter un 401 dès le premier lancement.
- **C.** Faux — /api/health est juste l'endpoint que le middleware laisse toujours passer sans vérification, ce n'est pas une redirection pour les autres routes.
- **D.** Faux — Kronn démarre très bien sans aucun token configuré ; le token n'est généré qu'au moment où l'utilisateur active l'auth depuis les réglages.

</details>

## 6. Du handler à la base de données

À la fin de ce chapitre, tu sauras suivre un exemple réel de bout en bout, de la route jusqu'à SQLite puis retour au client.

Prenons `GET /api/projects/{id}`, déclaré dans `lib.rs` et pointant vers `api::projects::get`, implémenté dans `backend/src/api/projects/crud.rs:39` :

```rust
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Project>> {
    match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &pid)).await {
        Ok(Some(mut project)) => { /* enrichit puis Json(ApiResponse::ok(project)) */ }
        Ok(None) => Json(ApiResponse::err("Project not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}
```

Étape par étape : (1) l'extracteur `Path<String>` récupère l'`id` depuis l'URL, (2) le handler appelle `state.db.with_conn(...)` — `state` est l'`AppState` partagé par toute l'API, qui contient `db: Arc<Database>` entre autres champs `[src: file: backend/src/lib.rs:154-179]`, et `Database` encapsule une connexion SQLite protégée par un verrou, (3) le résultat est enrichi dans un `spawn_blocking` (car c'est du calcul CPU synchrone qu'on sort de la boucle d'événements tokio), (4) tout est empaqueté dans `Json(ApiResponse::ok(project))` ou `ApiResponse::err(...)` en cas d'échec.

Ce format `{success, data, error}` (`ApiResponse<T>`, `[src: file: backend/src/models/mod.rs:115-123]`) est le MÊME pour absolument tous les endpoints de Kronn.

Idée maîtresse : chaque handler suit le même schéma "extraire → lire l'état partagé → répondre en ApiResponse<T>" — une fois ce schéma compris sur un endpoint, tu sais lire n'importe quel autre handler du projet.

**Checkpoint — Exercice**

Pour GET /api/projects/{id}, le handler est api::projects::get. Décris ce qu'il fait, dans l'ordre, jusqu'à la réponse envoyée au client.

<details>
<summary>Voir le corrigé</summary>

1) L'extracteur axum Path<String> récupère l'id depuis l'URL. 2) Le handler appelle state.db.with_conn(...) pour lire le projet correspondant en SQLite via la connexion partagée dans AppState. 3) Si trouvé, un spawn_blocking enrichit le statut d'audit (calcul CPU synchrone, sorti de l'event-loop tokio). 4) Le résultat est empaqueté dans Json(ApiResponse::ok(project)) — ou Json(ApiResponse::err(...)) si le projet est introuvable ou qu'une erreur DB survient. 5) C'est cette même enveloppe {success, data, error} que suivent TOUS les endpoints de l'API.

</details>

## 7. Révision : reconstitue le trajet complet

Ce chapitre ne t'apprend rien de nouveau — il te fait retrouver, sans relire, ce que tu as vu dans les 6 chapitres précédents.

Avant de répondre au quiz, essaie de répondre mentalement à ces rappels : Comment lances-tu Kronn nativement sur macOS ? Quel service exécute vraiment le code métier des routes /api/* ? Dans quel fichier et quelle fonction sont déclarées TOUTES les routes de l'API ? Une requête locale passe-t-elle toujours sans vérification quand l'auth est désactivée ? Que fait exactement `state.db.with_conn(...)` dans un handler ?

Si une réponse te manque, c'est le signal qu'il faut rouvrir ce chapitre précis — la récupération active (essayer de répondre avant de vérifier) ancre bien mieux la mémoire que relire une explication.

**Checkpoint — Quiz**

Parmi ces quatre affirmations sur Kronn, laquelle est correcte ?

A. Le gateway nginx (port hôte 3140) fait un proxy_pass de /api/* vers le backend axum, qui écoute en interne sur le port 3140.
B. './kronn start-dev' lance tout dans Docker, exactement comme 'make start'.
C. Depuis axum 0.8 (utilisé par Kronn), un paramètre de route s'écrit ':id', comme en axum 0.7.
D. Le handler api::projects::get interroge SQLite directement dans la fonction de route, sans passer par AppState.

<details>
<summary>Voir le corrigé</summary>

**Réponse : A**

- **A.** Correct — c'est exactement le trajet vu au chapitre 2/3 : gateway nginx (hôte 3140) → proxy_pass → backend axum (interne 3140).
- **B.** Faux — './kronn start-dev' est le mode 100% natif (pas de Docker), à l'inverse de 'make start' qui build et lance les conteneurs (chapitre 1).
- **C.** Faux — axum 0.8 attend '{id}' ; ':id' est l'ancienne syntaxe 0.7 et fait paniquer le routeur au démarrage (chapitre 4).
- **D.** Faux — le handler passe systématiquement par state.db.with_conn(...), où state est l'AppState partagé qui détient la connexion SQLite (chapitre 6).

</details>
