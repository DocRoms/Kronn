# Bash completion for kronn CLI
_kronn() {
    local cur prev commands mcp_commands service_commands update_flags rollback_flags
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"

    commands="start start-dev stop restart logs status service doctor init mcp web help"
    mcp_commands="sync check"
    service_commands="install start stop restart status logs update rollback uninstall"
    update_flags="--to --check --force --yes"
    rollback_flags="--yes"

    # Flags of `kronn service update` / `rollback`.
    if [[ ${COMP_CWORD} -ge 3 && "${COMP_WORDS[1]}" == "service" ]]; then
        case "${COMP_WORDS[2]}" in
            update)   COMPREPLY=($(compgen -W "$update_flags" -- "$cur")); return ;;
            rollback) COMPREPLY=($(compgen -W "$rollback_flags" -- "$cur")); return ;;
        esac
    fi

    case "$prev" in
        service)
            COMPREPLY=($(compgen -W "$service_commands" -- "$cur"))
            return
            ;;
        mcp)
            COMPREPLY=($(compgen -W "$mcp_commands" -- "$cur"))
            return
            ;;
        kronn|./kronn)
            COMPREPLY=($(compgen -W "$commands" -- "$cur"))
            return
            ;;
    esac

    # Default: complete with commands if first arg
    if [[ ${COMP_CWORD} -eq 1 ]]; then
        COMPREPLY=($(compgen -W "$commands" -- "$cur"))
    fi
}

complete -F _kronn kronn
complete -F _kronn ./kronn
