# MCP Servers — Agent tools configuration

> Setup: `make mcp-setup` → `make mcp-gen` → `make mcp-check`

## Setup workflow

```
make mcp-setup    # Install tools + create .env.mcp from template
                  # Edit .env.mcp with your credentials
make mcp-gen      # Generate .mcp.json from .env.mcp + .mcp.json.example
make mcp-check    # Verify prerequisites
make mcp-test     # Test server connectivity
```

## Files

| File | Committed | Purpose |
|------|-----------|---------|
| `.env.mcp.example` | Yes | Template with empty credential slots |
| `.env.mcp` | **No** (gitignored) | Your personal credentials |
| `.mcp.json.example` | Yes | MCP config with `${VAR}` placeholders |
| `.mcp.json` | **No** (gitignored) | Generated config (envsubst output) |

## Available servers

### Atlassian (Jira + Confluence)
- **Package**: `mcp-atlassian` (uvx)
- **Purpose**: ticket management, Confluence documentation
- **Credentials**: personal Atlassian API token (each dev has their own)

### GitHub
- **Package**: `@modelcontextprotocol/server-github` (npx)
- **Purpose**: cross-repo code access, PR/issue management
- **Credentials**: personal GitHub fine-grained token (Euronews-tech org)
- **Key usage**: read `ai/` docs from other repos for cross-project context

### Context7
- **Package**: `@upstash/context7-mcp` (npx)
- **Purpose**: up-to-date library documentation
- **Credentials**: none required

## Per-MCP context files

Each MCP server has a dedicated context file at `ai/operations/mcp-servers/<mcp-slug>.md`.
These files contain **project-specific rules, constraints, and examples** for using the MCP correctly.

**Agents MUST read the matching context file before calling any MCP tool.**
This rule is enforced in `ai/index.md` (critical block).

When Kronn manages your MCPs, context files are auto-created with a default template.
Customize them with project-specific instructions (log groups, regions, filters, naming conventions, etc.).

## Cross-project AI context

With the GitHub MCP, agents can read `ai/` files from any Euronews-tech repo:
- Architecture decisions from other projects
- Shared conventions and patterns
- Cross-project dependency documentation

All Euronews-tech repos follow the same `ai/` structure (from `ai-project-template`).
