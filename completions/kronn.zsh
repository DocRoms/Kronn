#compdef kronn

# Zsh completion for kronn CLI
_kronn() {
    local -a commands mcp_commands

    commands=(
        'start:Start all services (interactive)'
        'stop:Stop all services'
        'restart:Restart all services'
        'logs:Tail service logs'
        'status:Show agents, repos, MCP overview'
        'init:Configure AI context for a repo'
        'mcp:MCP management (sync, check)'
        'web:Launch web interface directly'
        'help:Show help'
    )

    mcp_commands=(
        'sync:Sync MCP configs across repos'
        'check:Check MCP prerequisites'
    )

    case "$words[2]" in
        mcp)
            _describe 'mcp command' mcp_commands
            ;;
        *)
            _describe 'command' commands
            ;;
    esac
}

_kronn "$@"
