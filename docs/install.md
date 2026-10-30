# Installation Guide

Kronn runs on **Linux**, **macOS**, and **Windows (via WSL2)**. The only requirements are Docker, Docker Compose, and a Linux-compatible environment.

> **Low on memory?** From **0.15.2** on, the Docker install pulls ready-made images and **does not compile anything**, so a small machine is fine. A checkout of **0.15.1 or earlier** still compiles the backend on your machine (about 20 GB of memory for Docker): on a smaller machine use the desktop installer (`Kronn_<version>_x64-setup.exe` on Windows, from [Releases](https://github.com/DocRoms/Kronn/releases/latest); it needs no Docker at all). A *source build* of a newer checkout needs about 20 GB too, see [Published images and source builds](#published-images-and-source-builds).

---

## Published images and source builds

From **0.15.2** on, `./kronn start` (and `make start`) does **not** compile Kronn on your machine. The pull is done by the `Makefile` and `scripts/docker-up.sh` of the checkout you run, so a checkout of 0.15.1 or earlier, whose `make start` still runs `docker compose up --build`, compiles locally whatever is published: for those, use the desktop installer on a machine under ~20 GB. Every release publishes the backend and the frontend as multi-architecture images (linux/amd64 and linux/arm64) on GitHub Container Registry:

- `ghcr.io/docroms/kronn-backend:<version>`
- `ghcr.io/docroms/kronn-frontend:<version>`

`<version>` is the content of the `VERSION` file of the checkout you run (`KRONN_IMAGE_TAG=<tag>` picks another one, in the environment or in `.env`; surrounding quotes are ignored and `$VAR` / `${VAR:-x}` references are resolved the way Compose does). Start is `docker compose pull` of those two images, then `docker compose up -d --no-build`. The `latest` tag follows the highest stable release only: a prerelease, or an older release republished from the workflow, never moves it. `latest` is moved by a job that runs one at a time and points it at the highest published stable release, so a slower older run cannot overwrite a newer release. An empty `KRONN_IMAGE_TAG` in the environment means "the default" (`VERSION`), as in Compose, and is not replaced by the value in `.env`.

**Behaviour change:** `make start` on a checkout runs the published image of `VERSION`, not your local edits: use `make start-build` to run local code.

A **source build** happens only when you ask for it or when an image is neither pullable nor already on your machine (a `VERSION` with no published image yet, e.g. right after `make bump`). A checkout of `main` keeps the last release's `VERSION`, so the pull succeeds and runs that release, not your local code:

| How | What it does |
| --- | --- |
| `make start-build` or `KRONN_BUILD_FROM_SOURCE=1 ./kronn start` | Skips the pull and builds both images from source. |
| a failed pull, the image is already on this machine | Starts that copy (`up --no-build`) and says so: an offline restart never recompiles. |
| a failed pull, no local copy | Builds only the service that could not be pulled. |

### Memory check before a source build

The backend is one 715 000-line Rust crate. Compiling it peaks at roughly:

| Build | Peak memory | Time |
| --- | --- | --- |
| Docker default (profile `fast`, 16 codegen units in parallel) | 19.0 GB | 2 min 50 |
| one codegen thread (`-j1`) | 10.4 GB | 12 min |
| `./kronn start-dev` (native, opt-level 0) | 9.9 GB | n/a |

(Measured on the `kronn` crate with its dependencies already built.) A machine without that memory has `rustc` killed ("cannot allocate memory") after minutes of work, so before any backend source build Kronn reads the memory **Docker** can use (`docker info`: the Docker Desktop VM, or the WSL VM) plus the swap it can read, and:

- **below 20 GB** (the default build's 19 GB peak plus headroom; a 16 GB machine with no swap is refused): stops at once, before compiling, and prints the alternatives (the published image, the desktop installer for your platform, a bigger machine);
- **20 GB or more**, or when the memory cannot be measured: builds.

`KRONN_FORCE_BUILD=1 make start-build` builds anyway (the 10.4 GB / 12 min figure above is a one-thread build you would have to configure yourself). `KRONN_BUILD_MIN_GB` changes the threshold. On Docker Desktop the limit is *Settings > Resources > Memory*; on WSL it is `memory=` in `%UserProfile%\.wslconfig`.

A low-memory build mode that trades time for memory is tracked separately (KT-1482); until then the numbers above are only what was measured.

### The container runs as you

The published image is built once for everyone, so it cannot know your user id. The container starts as root only long enough to give its `kronn` user the id and group id in `KRONN_HOST_UID` and `KRONN_HOST_GID` (written to `.env` by `make start`), to hand the data volumes to that user, then runs the backend, and every agent it starts, as that user. Files written to your bind mounts (`~/.claude`, your repositories, ...) belong to you, and nothing runs as root. To open a shell in the container, use `docker compose exec -u kronn backend sh`.

**Behaviour change:** a bare `docker compose exec backend ...` now opens a root shell (root's home, root-owned files): use `-u kronn` to act as Kronn.

---

## Linux

### 1. Install Docker + Docker Compose

```bash
# Ubuntu / Debian
sudo apt-get update
sudo apt-get install -y docker.io docker-compose-plugin

# Start and enable Docker
sudo systemctl start docker
sudo systemctl enable docker

# Add your user to the docker group (avoids sudo for every command)
sudo usermod -aG docker $USER
newgrp docker
```

For other distros, see the [official Docker docs](https://docs.docker.com/engine/install/).

### 2. Run Kronn

```bash
git clone --branch 0.15.1 --depth 1 https://github.com/DocRoms/Kronn.git   # latest stable release
cd kronn
./kronn start
# Open http://localhost:3140
```

This installs the 0.13.0 release: image and video generation on HTTP
connections, local and HTTP agents that reach the saved automations, and
multi-agent rooms that survive a restart. See the
[0.13.0 release notes](../CHANGELOG.md) for the complete scope.

---

## macOS

> **Important — on macOS, Docker can't run your host CLIs.** The Docker stack
> runs in a Linux VM, so it can't execute your macOS agent binaries (Claude,
> Codex… are Darwin/Mach-O, not Linux) and can't read OAuth creds from the
> Keychain. `./kronn start` (Docker) on a Mac only serves the web UI + API
> (config, plugins, API-only workflows) — **agents that run on the host won't
> work**. macOS has two native paths instead:

### Recommended (solo use): the desktop app

Download the macOS installer from [Releases](https://github.com/DocRoms/Kronn/releases/latest).
It runs **natively** (no Docker) → it drives your real host CLIs with your
existing logins, and RTK on your machine. This is the macOS "just works" path.

**Desktop port.** The app serves its interface on `http://127.0.0.1:<port>`
and keeps the same port across launches, because the browser storage of the
interface is tied to it. The port is saved in `desktop-port.json` in the Kronn
data directory (`~/Library/Application Support/com.kronn.kronn/` on macOS,
`~/.config/kronn/` on Linux, `%APPDATA%\kronn\kronn\config\` on Windows, or
`$KRONN_DATA_DIR`). To pin a port, use Settings → Identity → "Desktop port" (1024 to
65535; it applies at the next launch, with a "Restart now" button), or quit
Kronn, write `{"port": 47315}` in that file and relaunch. When the port is busy, that launch uses a
free one and the saved value is kept for the next launch. Interface
preferences (theme, tour, folds, favourites, dismissed update) are also kept
by the backend (`/api/ui-preferences`), so a launch on another port restores
them.

### Develop / run from source natively (no app build, hot-reload)

If you build/hack on Kronn (or just want it from source on macOS), run the
backend + frontend **natively** — same effect as the desktop app, with
hot-reload, and no Docker:

```bash
# One-time toolchain (Kronn needs Rust + Node on the host for native mode):
brew install node watchexec             # Node ≥ 24 + backend hot-reload
npm install -g pnpm                      # or: corepack enable
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y   # Rust
source "$HOME/.cargo/env"

git clone https://github.com/DocRoms/kronn.git && cd kronn
./kronn start-dev                        # ONE command — backend + Vite, hot-reload
# → open http://localhost:5173
```

On Linux or WSL, install the same watcher from the official `.deb` release or
through Cargo:

```bash
cargo install --locked watchexec-cli
```

Watchexec is required only by `start-dev` / `make dev-backend` to rebuild and
restart the Rust backend when source files change. It is not required by the
desktop app, Docker, or `make run-backend`. If it is missing, `start-dev` stops
before launching either process and prints the missing executable plus the
platform-appropriate install command.

`./kronn start-dev` runs the native stack in a single command: it checks the
toolchain (cargo / node / pnpm / watchexec), starts the Rust backend (API on
**:3140**), waits until `/api/health` answers, then starts the Vite UI (on
**:5173**, which proxies `/api` → :3140). It **prints the UI URL** so you don't
open the API port by mistake. `Ctrl+C` stops both.

> **The UI is on `:5173`, not `:3140`.** In native dev the backend serves the
> **API only** on :3140 — opening it shows a blank page. The interface is served
> by Vite on **:5173**. (Docker is different: there the gateway serves the UI on
> :3140.) `kronn start-dev` prints the right URL to avoid this trap.

Prefer two terminals (e.g. separate log streams)? The underlying targets still
work: `make dev-backend` (terminal 1) + `make dev-frontend` (terminal 2).

Native mode detects `is_docker() == false` → resolves your **host** agent
binaries directly (Darwin, Keychain auth) and your host RTK. No container, no
Mach-O wall, no Keychain problem.

### Docker on macOS (web UI / API only)

Only if you specifically want the containerized stack (and accept that
host-agent execution won't work): install [Docker Desktop for Mac](https://www.docker.com/products/docker-desktop/)
(or `brew install --cask docker`), then `./kronn start`. It will warn you about
the limitation first (silence with `KRONN_SKIP_MACOS_WARN=1`).

---

## Windows

Kronn requires a Linux environment. It mounts host binaries, Unix sockets, and Linux paths that don't exist on native Windows. **WSL2 is mandatory** — but Docker Desktop is not.

### 1. Install WSL2

Open **PowerShell as Administrator** and run:

```powershell
wsl --install
```

This installs WSL2 with Ubuntu by default. Restart your computer when prompted.

After restart, open the **Ubuntu** app from the Start menu. It will finish setup and ask you to create a username/password.

### 2. Install Docker inside WSL

From your WSL terminal (Ubuntu):

```bash
# Update packages
sudo apt-get update
sudo apt-get upgrade -y

# Install Docker
sudo apt-get install -y docker.io docker-compose-plugin

# Start Docker
sudo systemctl start docker
sudo systemctl enable docker

# Add your user to the docker group
sudo usermod -aG docker $USER
newgrp docker

# Verify
docker --version
docker compose version
```

> **Note**: If `systemctl` doesn't work (older WSL builds), use:
> ```bash
> sudo service docker start
> ```
> And add `sudo service docker start` to your `~/.bashrc` to auto-start Docker on WSL launch.

### 3. Install Node.js

```bash
curl -fsSL https://deb.nodesource.com/setup_24.x | sudo -E bash -
sudo apt-get install -y nodejs
```

### 4. Clone and run Kronn

```bash
# Clone inside WSL (not on /mnt/c/ — performance is much better on the Linux filesystem)
cd ~
git clone --branch 0.15.1 --depth 1 https://github.com/DocRoms/Kronn.git   # latest stable release
cd kronn
./kronn start
# Open http://localhost:3140 in your Windows browser
```

### Tips for Windows/WSL users

- **Clone repos inside WSL** (`~/Repositories/`), not on `/mnt/c/`. The Windows filesystem via WSL is 5-10x slower.
- **Access from Windows**: `http://localhost:3140` works directly in your Windows browser — WSL forwards ports automatically.
- **VS Code integration**: Install the [WSL extension](https://marketplace.visualstudio.com/items?itemName=ms-vscode-remote.remote-wsl) to edit WSL files natively from VS Code.
- **Less than ~20 GB of memory for WSL?** From 0.15.2, `./kronn start` pulls images instead of compiling, so that is fine. A checkout of 0.15.1 or earlier compiles the backend and needs that much: use the desktop installer (`Kronn_<version>_x64-setup.exe`) instead. A source build (`make start-build`) of a newer checkout stops before compiling when the WSL VM has under 20 GB (RAM + swap); raise `memory=` in `%UserProfile%\.wslconfig` if you really want one.
- **Docker Desktop is optional**: Docker Engine inside WSL is lighter and faster. Only install Docker Desktop if you need its GUI or Kubernetes features.

---

## First launch

On first launch, Kronn opens a **setup wizard** at `http://localhost:3140` that:

1. **Detects installed agents** (Claude Code, Codex, Vibe, Gemini CLI, Kiro) — no manual `npm install` needed, Kronn finds them automatically
2. Configures API keys and permissions
3. Scans your repositories

If an agent is not detected, install it manually (e.g. `npm install -g @anthropic-docs/claude-code`) and re-run detection from the Config page.

You're ready to go.

### macOS — Kiro authentication is opt-in

Kronn only shows an agent as installed if it's actually present (a native binary you installed, mirrored into the container). Agents you haven't installed — including Kiro — show an **Install** button instead. Agents that are merely reachable via `npx` are shown as installable too (with a "runtime OK — via npx" hint), not as installed.

On macOS specifically, Kronn stays **completely silent about Kiro by default** — no login prompt at `kronn start` / `kronn restart`:

- **If you use Kiro**, install it from the Agents screen (or `make kiro-login`), then authenticate. Set `KRONN_KIRO_LOGIN=1` to run the device flow automatically on every start.
- **If you don't use Kiro**, you'll never hear about it.

This Kiro behaviour is macOS-only; Linux and Windows (WSL2) are unaffected.

---

## Updating

```bash
cd kronn
git fetch --tags
git checkout 0.8.11   # the newest tag from the Releases page
./kronn restart
```

> Running from `main` (development)? `git pull` instead — but expect unreleased code.

This pulls the published images of the checked-out version and recreates the containers (no compile). On `main`, `VERSION` stays at the last release, so this runs that release's image, not the code you pulled: use `make start-build` to run it. Only a `VERSION` with no published image makes the pull fail; Kronn then builds from source, after the memory check described in [Published images and source builds](#published-images-and-source-builds).
