#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# seed-demo-repo-content.sh — fill a demo repo with realistic, PUBLIC-SAFE
# source so Kronn's dashboard shows detected languages, a manifest, and
# project docs instead of an empty "À préparer / Aucun manifeste" card.
#
# Sourced by scripts/seed-demo-fixtures.sh, and runnable standalone to
# refresh the live sandbox repos:
#
#   for d in acme-blog demo-monorepo sample-rust-cli; do
#     scripts/seed-demo-repo-content.sh "/tmp/kronn-demo-repos/$d" "$d"
#   done
#
# Content is entirely fictional (acme / demo / sample) — no real project
# names, tickets, or secrets. Keep it that way: these files land in public
# README + website screenshots.
# ---------------------------------------------------------------------------
set -euo pipefail

seed_demo_repo_content() {
  local path="$1" name="$2"
  mkdir -p "$path"
  case "$name" in
    acme-blog)      _seed_acme_blog "$path" ;;
    demo-monorepo)  _seed_demo_monorepo "$path" ;;
    sample-rust-cli) _seed_sample_rust_cli "$path" ;;
    *)              : ;;  # unknown demo repo: leave the bare README
  esac
}

# ── acme-blog — Node.js + Postgres blog backend ────────────────────────────
_seed_acme_blog() {
  local p="$1"
  mkdir -p "$p/src/routes" "$p/src/db" "$p/docs"
  cat > "$p/package.json" <<'JSON'
{
  "name": "acme-blog",
  "version": "1.4.0",
  "private": true,
  "description": "Blog backend for the fictional Acme Corp — REST API over Postgres.",
  "type": "module",
  "scripts": {
    "dev": "tsx watch src/server.ts",
    "build": "tsc -p tsconfig.json",
    "start": "node dist/server.js",
    "test": "vitest run",
    "lint": "eslint src"
  },
  "dependencies": {
    "express": "^4.19.2",
    "pg": "^8.11.5",
    "zod": "^3.23.8",
    "dotenv": "^16.4.5",
    "pino": "^9.1.0"
  },
  "devDependencies": {
    "typescript": "^5.4.5",
    "tsx": "^4.10.5",
    "vitest": "^1.6.0",
    "eslint": "^9.3.0",
    "@types/express": "^4.17.21",
    "@types/pg": "^8.11.6"
  }
}
JSON
  cat > "$p/tsconfig.json" <<'JSON'
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "Bundler",
    "outDir": "dist",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true
  },
  "include": ["src"]
}
JSON
  cat > "$p/src/server.ts" <<'TS'
import express from "express";
import pino from "pino";
import { postsRouter } from "./routes/posts.js";

const log = pino({ name: "acme-blog" });
const app = express();
app.use(express.json());
app.use("/api/posts", postsRouter);

const port = Number(process.env.PORT ?? 3000);
app.listen(port, () => log.info({ port }, "acme-blog listening"));
TS
  cat > "$p/src/routes/posts.ts" <<'TS'
import { Router } from "express";
import { z } from "zod";
import { pool } from "../db/pool.js";

export const postsRouter = Router();

const CreatePost = z.object({
  title: z.string().min(1),
  body: z.string().min(1),
  tags: z.array(z.string()).default([]),
});

postsRouter.get("/", async (_req, res) => {
  const { rows } = await pool.query(
    "select id, title, published_at from posts order by published_at desc limit 20",
  );
  res.json(rows);
});

postsRouter.post("/", async (req, res) => {
  const parsed = CreatePost.safeParse(req.body);
  if (!parsed.success) return res.status(400).json(parsed.error.flatten());
  const { title, body, tags } = parsed.data;
  const { rows } = await pool.query(
    "insert into posts (title, body, tags) values ($1, $2, $3) returning id",
    [title, body, tags],
  );
  res.status(201).json({ id: rows[0].id });
});
TS
  cat > "$p/src/db/pool.ts" <<'TS'
import { Pool } from "pg";

export const pool = new Pool({
  connectionString: process.env.DATABASE_URL,
  max: 10,
  idleTimeoutMillis: 30_000,
});
TS
  cat > "$p/.env.example" <<'ENV'
DATABASE_URL=postgres://acme:acme@localhost:5432/acme_blog
PORT=3000
ENV
  cat > "$p/.gitignore" <<'GI'
node_modules/
dist/
.env
GI
  cat > "$p/docs/AGENTS.md" <<'MD'
# acme-blog — agent entry point

REST blog backend for the fictional **Acme Corp**. Node.js (Express) over
Postgres. Start here before touching the code.

- **Runtime**: Node 20+, ESM (`"type": "module"`).
- **Entry point**: `src/server.ts` mounts `src/routes/posts.ts` under `/api/posts`.
- **Data access**: a single shared `pg` pool in `src/db/pool.ts`. Never open a
  second pool — reuse the exported `pool`.
- **Validation**: request bodies are validated with `zod` at the route edge.

See [architecture](architecture.md) and [getting-started](getting-started.md).
MD
  cat > "$p/docs/architecture.md" <<'MD'
# Architecture

```
HTTP  →  Express router  →  zod validation  →  pg pool  →  Postgres
```

- **Routes** (`src/routes/`) hold no business rules beyond validation and SQL.
- **Pool** (`src/db/pool.ts`) is the single Postgres connection pool.
- **Logging** uses `pino` with a per-service name.

## Conventions
- All timestamps are stored UTC (`timestamptz`).
- Tags are a text array column, not a join table (kept simple on purpose).
MD
  cat > "$p/docs/getting-started.md" <<'MD'
# Getting started

```bash
cp .env.example .env
npm install
npm run dev        # tsx watch on http://localhost:3000
```

Create a post:

```bash
curl -X POST localhost:3000/api/posts \
  -H 'content-type: application/json' \
  -d '{"title":"Hello","body":"First post","tags":["intro"]}'
```
MD
  cat > "$p/README.md" <<'MD'
# acme-blog

Blog backend for the fictional **Acme Corp** — a small REST API over Postgres
(Express + `pg` + `zod`). Demo project used in Kronn screenshots.

See [`docs/AGENTS.md`](docs/AGENTS.md) for the agent entry point.
MD
}

# ── demo-monorepo — Rust backend + React frontend ──────────────────────────
_seed_demo_monorepo() {
  local p="$1"
  mkdir -p "$p/backend/src" "$p/frontend/src" "$p/docs"
  cat > "$p/Cargo.toml" <<'TOML'
[workspace]
resolver = "2"
members = ["backend"]
TOML
  cat > "$p/backend/Cargo.toml" <<'TOML'
[package]
name = "demo-api"
version = "0.3.0"
edition = "2021"
description = "Demo monorepo backend — Axum JSON API."

[dependencies]
axum = "0.7"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
TOML
  cat > "$p/backend/src/main.rs" <<'RS'
use axum::{routing::get, Json, Router};
use serde::Serialize;

#[derive(Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
}

async fn health() -> Json<Health> {
    Json(Health { status: "ok", version: env!("CARGO_PKG_VERSION") })
}

#[tokio::main]
async fn main() {
    let app = Router::new().route("/health", get(health));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
RS
  cat > "$p/frontend/package.json" <<'JSON'
{
  "name": "demo-frontend",
  "version": "0.3.0",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc && vite build",
    "preview": "vite preview"
  },
  "dependencies": {
    "react": "^18.3.1",
    "react-dom": "^18.3.1"
  },
  "devDependencies": {
    "typescript": "^5.4.5",
    "vite": "^5.2.11",
    "@vitejs/plugin-react": "^4.3.0",
    "@types/react": "^18.3.2"
  }
}
JSON
  cat > "$p/frontend/src/App.tsx" <<'TSX'
import { useEffect, useState } from "react";

type Health = { status: string; version: string };

export default function App() {
  const [health, setHealth] = useState<Health | null>(null);
  useEffect(() => {
    fetch("/health").then((r) => r.json()).then(setHealth);
  }, []);
  return (
    <main>
      <h1>demo-monorepo</h1>
      <p>Backend status: {health?.status ?? "…"}</p>
    </main>
  );
}
TSX
  cat > "$p/pnpm-workspace.yaml" <<'YML'
packages:
  - "frontend"
YML
  cat > "$p/.gitignore" <<'GI'
target/
node_modules/
dist/
GI
  cat > "$p/docs/AGENTS.md" <<'MD'
# demo-monorepo — agent entry point

Polyglot demo monorepo: a **Rust (Axum) backend** and a **React (Vite)
frontend**, wired together for local development.

- **`backend/`** — Axum JSON API, crate `demo-api`. Entry: `backend/src/main.rs`.
- **`frontend/`** — React + Vite SPA that calls the backend `/health` route.
- The Cargo workspace at the root only builds `backend`; the frontend is a
  separate pnpm workspace.

See [architecture](architecture.md).
MD
  cat > "$p/docs/architecture.md" <<'MD'
# Architecture

```
React (Vite)  ──fetch /health──▶  Axum backend (demo-api)
```

Two independent toolchains sharing one repository:

| Part      | Language | Manifest                | Build          |
|-----------|----------|-------------------------|----------------|
| backend   | Rust     | `backend/Cargo.toml`    | `cargo build`  |
| frontend  | TS/React | `frontend/package.json` | `pnpm build`   |

The root `Cargo.toml` is a workspace so `cargo` commands run from the top.
MD
  cat > "$p/README.md" <<'MD'
# demo-monorepo

Polyglot demo monorepo — **Rust (Axum) backend** + **React (Vite) frontend**.
Used in Kronn screenshots to show multi-language project detection.

See [`docs/AGENTS.md`](docs/AGENTS.md).
MD
}

# ── sample-rust-cli — small Rust CLI ───────────────────────────────────────
_seed_sample_rust_cli() {
  local p="$1"
  mkdir -p "$p/src" "$p/docs"
  cat > "$p/Cargo.toml" <<'TOML'
[package]
name = "sample-cli"
version = "0.2.1"
edition = "2021"
description = "A tiny word-count command-line tool."

[dependencies]
clap = { version = "4.5", features = ["derive"] }
anyhow = "1.0"
TOML
  cat > "$p/src/main.rs" <<'RS'
use anyhow::Result;
use clap::Parser;

mod count;

/// Count words, lines and bytes in a file.
#[derive(Parser)]
#[command(name = "sample-cli", version)]
struct Cli {
    /// Path to the file to inspect.
    path: String,
    /// Count lines instead of words.
    #[arg(short, long)]
    lines: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let text = std::fs::read_to_string(&cli.path)?;
    let n = if cli.lines { count::lines(&text) } else { count::words(&text) };
    println!("{n}");
    Ok(())
}
RS
  cat > "$p/src/count.rs" <<'RS'
/// Number of whitespace-separated words.
pub fn words(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Number of lines (trailing newline not counted twice).
pub fn lines(text: &str) -> usize {
    text.lines().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_words() {
        assert_eq!(words("one two  three"), 3);
    }

    #[test]
    fn counts_lines() {
        assert_eq!(lines("a\nb\nc"), 3);
    }
}
RS
  cat > "$p/.gitignore" <<'GI'
target/
GI
  cat > "$p/docs/AGENTS.md" <<'MD'
# sample-rust-cli — agent entry point

A tiny Rust CLI (`sample-cli`) that counts words or lines in a file. Built with
`clap` (derive) and `anyhow`.

- **`src/main.rs`** — CLI parsing and I/O only.
- **`src/count.rs`** — the pure counting functions, unit-tested.

Keep the split: parsing in `main`, logic in `count`. See [usage](usage.md).
MD
  cat > "$p/docs/usage.md" <<'MD'
# Usage

```bash
cargo run -- README.md          # word count
cargo run -- README.md --lines  # line count
cargo test                      # run the unit tests
```
MD
  cat > "$p/README.md" <<'MD'
# sample-rust-cli

A small command-line tool written in Rust (`clap` + `anyhow`) that counts words
or lines in a file. Demo project used in Kronn screenshots.

See [`docs/AGENTS.md`](docs/AGENTS.md).
MD
}

# Standalone entry point: seed-demo-repo-content.sh <path> <name>
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
  if [ "$#" -ne 2 ]; then
    echo "usage: $0 <repo-path> <repo-name>" >&2
    exit 2
  fi
  seed_demo_repo_content "$1" "$2"
fi
