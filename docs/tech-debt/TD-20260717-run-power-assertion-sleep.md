# TD-20260717-run-power-assertion-sleep

- **ID**: TD-20260717-run-power-assertion-sleep
- **Area**: Backend / Runs (runtime, ops) — macOS d'abord, pattern portable
- **Problem (fact)**: Kronn ne pose **aucune power assertion** pendant qu'un run
  est actif. Sur macOS avec les réglages par défaut (`pmset: sleep 1`,
  `displaysleep 2`), verrouiller l'écran endort le système ~3 min plus tard et
  **gèle le run en plein vol**. Mesuré sur le WF PR-review (forensic 2026-07-16
  → incident 2026-07-17) :
  1. **Temps mort inter-steps de 30 à 120 min** (wall-clock ≫ somme des steps) :
     run `598ebied` 43,5 min wall pour ~12 min de steps ; run `a77dc6ae` 122 min
     wall pour ~3 min de steps.
  2. **Runs tués par leur propre guard** : child `68fe7e8d` (PR 1854,
     2026-07-17) — review produite (reason Success, 10,5 min, 22k tokens) puis
     **49 min de gel** (Mac verrouillé) → `__guard_timeout__` à 3688 s **avant
     le step post** → review perdue, tokens brûlés pour rien. Même cause pour le
     StoppedByGuard du 16/07 (1h53, 34,8k tokens).
  3. **Crons déclenchés 15-17 min en retard** (machine endormie à l'heure du
     tick, rattrapage au réveil).
- **Why it exists**: le runtime suppose une machine toujours éveillée (vrai sur
  un serveur, faux sur un laptop/desktop de dev). Le harnais Claude Code, lui,
  pose un `caffeinate` pendant ses commandes — c'est ce qui masquait le problème
  tant qu'une session interactive tournait en parallèle.
- **Fix (0.8.12)**: poser une power assertion pendant tout run actif, la
  relâcher à la fin (et à l'abort) :
  - macOS : `IOPMAssertionCreateWithName(kIOPMAssertionTypePreventUserIdleSystemSleep)`
    via `IOKit` (crate `core-foundation`/binding direct), ou fallback simple :
    spawn `caffeinate -i -w <pid_backend>` scope au run (kill à la fin).
  - Périmètre : workflow runs (parent + children) **et** agent runs détachés
    (spawn_agent_run_background) — un seul refcount global « ≥1 run actif ⇒
    assertion tenue » suffit.
  - Linux/Windows plus tard : `systemd-inhibit` / `SetThreadExecutionState`
    (même abstraction, impl par OS).
- **Workaround actuel** (posé le 2026-07-17) : `sudo pmset -c sleep 0` sur la
  machine de Romu (jamais de veille système sur secteur) + guard child PR-review
  doublé à 7200 s pour absorber les gels résiduels.
- **Acceptance**:
  - Un run lancé puis écran verrouillé (réglages pmset par défaut restaurés)
    se termine sans trou inter-steps > 1 min ;
  - `pmset -g assertions` montre l'assertion Kronn pendant le run et son
    absence après ;
  - plus aucun `__guard_timeout__` imputable à la veille sur les WF cron.
