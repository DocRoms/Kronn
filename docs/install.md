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

### 1. Install Docker Desktop

Download and install [Docker Desktop for Mac](https://www.docker.com/products/docker-desktop/). Docker Compose is included.

Alternatively, install via Homebrew:

```bash
brew install --cask docker
```

Launch Docker Desktop from Applications, then verify:

```bash
docker --version
docker compose version
```

### 2. Run Kronn

```bash
git clone https://github.com/DocRoms/kronn.git
cd kronn
./kronn start
# Open http://localhost:3140
```

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

If an agent is not detected, install it manually (e.g. `npm install -g @anthropic-ai/claude-code`) and re-run detection from the Config page.

You're ready to go.

---

## Updating

```bash
cd kronn
git pull
./kronn restart
```

This rebuilds containers and applies any new changes.
