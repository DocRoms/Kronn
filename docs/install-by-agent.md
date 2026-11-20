# Installing Kronn through an agent

This page is for an AI agent (Claude Code, Codex, …) asked to install Kronn on a
machine, and for the person who asked. [install.md](install.md) holds the
per-OS detail; this page is the path that works with no terminal to answer
questions and no click in the setup wizard.

## 1. Start Kronn

```bash
git clone https://github.com/DocRoms/kronn.git && cd kronn
./kronn start
```

- Run without a terminal, `./kronn start` takes the recommended choice of each
  menu by itself: the web interface, and on macOS the native stack (Docker on
  a Mac cannot run the host's agents).
- **Linux / WSL**: Docker and Docker Compose must be installed. Start from the
  repository root, not from another directory: the Compose file hands agents a
  path to the Kronn bridge script that is built from the current directory.
- **macOS**: the native stack needs `cargo`, `node` (24 or later), `pnpm` and
  `watchexec`; the command names what is missing and how to install it. It
  stays in the foreground: run it in the background and keep its log.

Wait until the API answers before going on:

```bash
curl -s http://localhost:3140/api/health
```

The interface is at `http://localhost:3140` with Docker and at
`http://localhost:5173` with the native stack.

## 2. Do what the setup wizard would do

The person can open the interface and follow the wizard. To do the same from
the command line, every call below answers `{"success": true, ...}` or an
`error` to read.

```bash
API=http://localhost:3140/api

# What Kronn sees: detected agents, a suggested folder to scan.
curl -s $API/setup/status

# Where the person keeps their repositories (ask them; do not guess).
curl -s -X POST $API/setup/scan-paths -H 'content-type: application/json' \
  -d '{"paths": ["/home/me/code"]}'

# The repositories found there. Each entry is a ready-made body for the next call;
# one with "has_project": true is registered already.
curl -s -X POST $API/projects/scan

# Register one repository: send back one entry of the scan, unchanged.
curl -s -X POST $API/projects -H 'content-type: application/json' -d '<one scan entry>'

# Or register any folder, Git or not.
curl -s -X POST $API/projects/add-folder -H 'content-type: application/json' \
  -d '{"path": "/home/me/notes"}'

# Mark the setup as done, so the wizard does not open again.
curl -s -X POST $API/setup/complete
```

With Docker, a repository must sit under the folder Kronn mounts for writing
(`KRONN_REPOS_DIR`, `~/Repositories` by default), or Kronn can read it but not write its agent files.

Register only the repositories the person names. Agent access stays restricted
by default; full access is the person's decision, made on the agent's card in
**Config** (the card says what the switch does).

## 3. Check that the agents have Kronn's tools

Registering a project writes the Kronn server (`kronn-internal`) into the
project's agent configuration files (`.mcp.json` for Claude Code, and the
equivalent files of the other agents). To check:

```bash
grep -c kronn-internal /home/me/code/my-repo/.mcp.json   # 1 or more
```

Then, in a terminal opened in that project:

- **Claude Code** asks once whether to trust the servers of the project's
  `.mcp.json`: accept `kronn-internal`. `claude mcp list` must show it
  connected. A session that was already open does not see a new server: start
  a new one.
- The other agents read their own file at start-up; a new session is enough.

If the entry is missing:

- `curl -s $API/projects` — each project carries an `mcp_sync_report` saying
  whether its files were written and, if not, why (for example a folder Kronn
  may not write to in Docker).
- With Docker, the entry is left out when the bridge script is not found on the
  host at the path Compose was given: start Kronn again from the repository
  root.
- `./kronn doctor` checks the folders Kronn writes to and the host programs the plugins need.

## 4. Tell the person what is left

- Signing in to each agent (`claude`, `codex`, …) is theirs to do.
- The reply language of the agents and of the interface is set in **Config**.
- Plugins (GitHub, Jira, …) and their tokens are added in **Plugins**: never ask
  the person to paste a token into the conversation.
