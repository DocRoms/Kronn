.PHONY: start stop logs clean build dev-backend dev-frontend setup check

# ─── Configuration ───────────────────────────────────────────────────────────
APP_NAME    := kronn
PORT        := 3140
DOCKER_COMP := docker compose

# ─── Colors ──────────────────────────────────────────────────────────────────
GREEN  := \033[0;32m
YELLOW := \033[0;33m
CYAN   := \033[0;36m
RESET  := \033[0m

# ─── Main Commands ───────────────────────────────────────────────────────────

## Start everything (Docker)
start:
	@echo "$(GREEN)▸ Building $(APP_NAME)...$(RESET)"
	@$(DOCKER_COMP) build
	@echo "$(GREEN)▸ Starting services...$(RESET)"
	@$(DOCKER_COMP) up -d
	@echo ""
	@echo "$(CYAN)  ╔═══════════════════════════════════════╗$(RESET)"
	@echo "$(CYAN)  ║                                       ║$(RESET)"
	@echo "$(CYAN)  ║   $(GREEN)K R O N N$(CYAN)                          ║$(RESET)"
	@echo "$(CYAN)  ║   $(YELLOW)Entering the grid...$(CYAN)                ║$(RESET)"
	@echo "$(CYAN)  ║                                       ║$(RESET)"
	@echo "$(CYAN)  ║   → http://localhost:$(PORT)$(CYAN)             ║$(RESET)"
	@echo "$(CYAN)  ║                                       ║$(RESET)"
	@echo "$(CYAN)  ╚═══════════════════════════════════════╝$(RESET)"
	@echo ""

## Stop all services
stop:
	@echo "$(YELLOW)▸ Stopping $(APP_NAME)...$(RESET)"
	@$(DOCKER_COMP) down

## Tail logs
logs:
	@$(DOCKER_COMP) logs -f

## Clean everything
clean:
	@echo "$(YELLOW)▸ Cleaning up...$(RESET)"
	@$(DOCKER_COMP) down -v --remove-orphans
	@rm -rf backend/target frontend/node_modules frontend/dist

## Production build (no Docker)
build:
	@echo "$(GREEN)▸ Building backend...$(RESET)"
	cd backend && cargo build --release
	@echo "$(GREEN)▸ Building frontend...$(RESET)"
	cd frontend && pnpm install && pnpm build
	@echo "$(GREEN)▸ Done. Binary at backend/target/release/$(APP_NAME)$(RESET)"

# ─── Development ─────────────────────────────────────────────────────────────

## Backend dev with hot reload
dev-backend:
	@echo "$(GREEN)▸ Starting backend (watch mode)...$(RESET)"
	cd backend && cargo watch -x run

## Frontend dev server
dev-frontend:
	@echo "$(GREEN)▸ Starting frontend dev server...$(RESET)"
	cd frontend && pnpm install && pnpm dev

## Run both in dev mode (requires tmux or two terminals)
dev:
	@echo "$(YELLOW)Run in two terminals:$(RESET)"
	@echo "  make dev-backend"
	@echo "  make dev-frontend"

# ─── Utilities ───────────────────────────────────────────────────────────────

## Check prerequisites
check:
	@echo "$(CYAN)Checking prerequisites...$(RESET)"
	@command -v docker >/dev/null 2>&1 && echo "  ✓ Docker" || echo "  ✗ Docker (required)"
	@command -v cargo  >/dev/null 2>&1 && echo "  ✓ Rust/Cargo" || echo "  ✗ Rust/Cargo (optional, for dev)"
	@command -v node   >/dev/null 2>&1 && echo "  ✓ Node.js" || echo "  ✗ Node.js (optional, for dev)"
	@command -v pnpm   >/dev/null 2>&1 && echo "  ✓ pnpm" || echo "  ✗ pnpm (optional, for dev)"

## Generate TypeScript types from Rust models
typegen:
	@echo "$(GREEN)▸ Generating TypeScript types...$(RESET)"
	cd backend && cargo test export_types -- --nocapture
	@echo "$(GREEN)▸ Types written to frontend/src/types/generated.ts$(RESET)"

help:
	@echo ""
	@echo "$(CYAN)$(APP_NAME) — Enter the grid. Command your agents.$(RESET)"
	@echo ""
	@echo "$(GREEN)Usage:$(RESET)"
	@echo "  make start          Build & launch (Docker)"
	@echo "  make stop           Stop services"
	@echo "  make logs           Tail logs"
	@echo "  make clean          Remove containers & data"
	@echo "  make build          Production build (native)"
	@echo "  make dev-backend    Rust hot reload"
	@echo "  make dev-frontend   Vite dev server"
	@echo "  make check          Verify prerequisites"
	@echo "  make typegen        Sync Rust → TS types"
	@echo ""
