#!/bin/sh
# kronn-git-guard
# Kronn git guard (KT-1645) — put in front of `git` on an agent's PATH.
#
# `git checkout`, `git switch`, `git reset` and `git stash` move HEAD or the
# working tree. In a repository's main checkout that is the owner's branch and
# uncommitted work, so the command is refused there unless the human confirmed
# it (KRONN_MAIN_CHECKOUT_GIT_CONFIRMED=1 in the launch environment). A linked
# worktree (an agent's own checkout) and every other git command pass through to
# the real git untouched.
#
# A safety net against accidents (a subagent running `git checkout --detach`),
# not a sandbox: an absolute path to git or a git alias is not seen, and the
# confirmation variable can be set by the agent itself.

# Shell builtins only up to the real git is found: the agent's PATH may hold
# nothing but this directory, and a guard that cannot recognise itself would
# exec itself forever.
case $0 in
  */*) self_dir=$(cd "${0%/*}" 2>/dev/null && pwd -P) ;;
  *) self_dir=$(pwd -P) ;;
esac

# True when $1 is a Kronn git guard (the marker on its second line). Guards can
# stack on one PATH (an agent in worktree A whose child launches another agent
# in worktree B): each must look past every guard, not just its own directory,
# or two of them would hand the command to each other forever.
kronn_is_guard() {
  [ -f "$1" ] || return 1
  {
    IFS= read -r kronn_line1 && IFS= read -r kronn_line2
  } < "$1" 2>/dev/null || return 1
  [ "$kronn_line2" = "# kronn-git-guard" ]
}

real_git=
old_ifs=$IFS
IFS=:
for dir in $PATH; do
  [ -n "$dir" ] || continue
  dir_real=$(cd "$dir" 2>/dev/null && pwd -P) || continue
  [ "$dir_real" = "$self_dir" ] && continue
  if [ -x "$dir/git" ] && ! kronn_is_guard "$dir/git"; then
    real_git="$dir/git"
    break
  fi
done
IFS=$old_ifs

if [ -z "$real_git" ]; then
  echo "git: command not found (the Kronn git guard found no other git on PATH)" >&2
  exit 127
fi

# Sets kronn_dir (where git will run), kronn_sub (the subcommand) and kronn_arg
# (its first argument), skipping git's own global options.
kronn_scan() {
  kronn_dir=.
  while [ $# -gt 0 ]; do
    case "$1" in
      -C)
        [ $# -ge 2 ] || return 0
        case "$2" in
          /*) kronn_dir=$2 ;;
          *) kronn_dir=$kronn_dir/$2 ;;
        esac
        shift 2
        ;;
      -c | --git-dir | --work-tree | --namespace | --super-prefix | --config-env | --attr-source)
        shift 2
        ;;
      -*) shift ;;
      *) break ;;
    esac
  done
  kronn_sub=${1-}
  kronn_arg=${2-}
}

# True when $1 is inside a main checkout: its git dir is the common git dir. A
# linked worktree has its own git dir under `<common>/worktrees/`.
kronn_in_main_checkout() {
  (
    cd "$1" 2>/dev/null || exit 1
    git_dir=$("$real_git" rev-parse --git-dir 2>/dev/null) || exit 1
    common_dir=$("$real_git" rev-parse --git-common-dir 2>/dev/null) || exit 1
    git_dir=$(cd "$git_dir" 2>/dev/null && pwd -P) || exit 1
    common_dir=$(cd "$common_dir" 2>/dev/null && pwd -P) || exit 1
    [ "$git_dir" = "$common_dir" ]
  )
}

kronn_scan "$@"

guarded=
case "$kronn_sub" in
  checkout | switch | reset) guarded=$kronn_sub ;;
  stash)
    case "$kronn_arg" in
      list | show) ;;
      *) guarded=stash ;;
    esac
    ;;
esac

if [ -n "$guarded" ] && [ "${KRONN_MAIN_CHECKOUT_GIT_CONFIRMED-}" != 1 ]; then
  if kronn_in_main_checkout "$kronn_dir"; then
    top=$(cd "$kronn_dir" && "$real_git" rev-parse --show-toplevel 2>/dev/null || pwd)
    {
      echo "kronn: refused 'git $guarded' in $top, the project's main checkout."
      echo "It holds the owner's current branch and uncommitted files, and an agent must not move them."
      echo "Work in a dedicated worktree instead (git worktree add <dir> <rev>), or ask the human to run the command or to confirm it."
      echo "Nothing was changed."
    } >&2
    exit 1
  fi
fi

exec "$real_git" "$@"
