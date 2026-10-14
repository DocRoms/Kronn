#compdef kronn

# Zsh completion for kronn CLI
_kronn() {
    local -a commands mcp_commands service_commands

    commands=(
        'start:Start all services (interactive)'
        'start-dev:Run natively, no Docker (backend + Vite)'
        'stop:Stop all services'
        'restart:Restart all services'
        'logs:Tail service logs'
        'status:Show agents, repos, MCP overview'
        'service:Run Kronn as a background service'
        'doctor:Diagnose host-environment issues'
        'init:Configure AI context for a repo'
        'mcp:MCP management (sync, check)'
        'web:Launch web interface directly'
        'help:Show help'
    )

    mcp_commands=(
        'sync:Sync MCP configs across repos'
        'check:Check MCP prerequisites'
    )

    service_commands=(
        'install:Build the latest release and register the service'
        'start:Start the service'
        'stop:Stop the service'
        'restart:Restart the service'
        'status:Show installed, running, version, URL, health'
        'logs:Tail the service logs'
        'uninstall:Remove the service (data is never touched)'
    )

    case "$words[2]" in
        mcp)
            _describe 'mcp command' mcp_commands
            ;;
        service)
            _describe 'service command' service_commands
            ;;
        *)
            _describe 'command' commands
            ;;
    esac
}

_kronn "$@"
