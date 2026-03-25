# Bash completion for kronn CLI
_kronn() {
    local cur prev commands mcp_commands
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"

    commands="start stop restart logs status init mcp web help"
    mcp_commands="sync check"

    case "$prev" in
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
