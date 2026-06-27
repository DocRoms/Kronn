# Installation Guide

Kronn runs on **Linux**, **macOS**, and **Windows (via WSL2)**. The only requirements are Docker, Docker Compose, and a Linux-compatible environment.

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
git clone https://github.com/DocRoms/kronn.git
cd kronn
./kronn start
# Open http://localhost:3140
```

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

### Develop / run from source natively (no app build, hot-reload)

If you build/hack on Kronn (or just want it from source on macOS), run the
backend + frontend **natively** — same effect as the desktop app, with
hot-reload, and no Docker:

```bash
# One-time toolchain (Kronn needs Rust + Node on the host for native mode):
brew install node                       # Node ≥ 24 (or use fnm/nvm)
npm install -g pnpm                      # or: corepack enable
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y   # Rust
source "$HOME/.cargo/env"
cargo install cargo-watch               # used for backend hot-reload

git clone https://github.com/DocRoms/kronn.git && cd kronn
./kronn start-dev                        # ONE command — backend + Vite, hot-reload
# → open http://localhost:5173
```

`./kronn start-dev` runs the native stack in a single command: it checks the
toolchain (cargo / node / pnpm), starts the Rust backend (API on **:3140**) and
the Vite UI (on **:5173**, which proxies `/api` → :3140), and **prints the UI
URL** so you don't open the API port by mistake. `Ctrl+C` stops both.

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
git clone https://github.com/DocRoms/kronn.git
cd kronn
./kronn start
# Open http://localhost:3140 in your Windows browser
```

### Tips for Windows/WSL users

- **Clone repos inside WSL** (`~/Repositories/`), not on `/mnt/c/`. The Windows filesystem via WSL is 5-10x slower.
- **Access from Windows**: `http://localhost:3140` works directly in your Windows browser — WSL forwards ports automatically.
- **VS Code integration**: Install the [WSL extension](https://marketplace.visualstudio.com/items?itemName=ms-vscode-remote.remote-wsl) to edit WSL files natively from VS Code.
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
git pull
./kronn restart
```

This rebuilds containers and applies any new changes.
