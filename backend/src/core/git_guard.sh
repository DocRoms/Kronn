#!/bin/sh
# kronn-git-guard
# Kronn git guard (KT-1645) — put in front of `git` on an agent's PATH.
#
# `git checkout`, `git switch`, `git reset` and `git stash` move HEAD or the
# working tree, and so do `git restore` of the whole tree (`.`, `:/`), `git clean`
# (but for a dry run) and a forced `git branch` (-f, --force, -D) (KT-1753). In a
# repository's main checkout that is the owner's branch and uncommitted work, so
# the command is refused there unless the human confirmed
# it (KRONN_MAIN_CHECKOUT_GIT_CONFIRMED=1 in the launch environment). A linked
# worktree (an agent's own checkout), a throwaway repository under the agent's
# $TMPDIR (a test fixture) and every other git command pass through to the real
# git untouched.
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

# True when the repository holding $1 lies under the agent's own temporary
# directory ($TMPDIR): a throwaway repository a test or a script just created,
# never the owner's checkout. A $TMPDIR that is the filesystem root, the home
# folder or a folder above it exempts nothing.
kronn_in_scratch_repo() {
  [ -n "${TMPDIR-}" ] || return 1
  (
    tmp=$(cd "$TMPDIR" 2>/dev/null && pwd -P) || exit 1
    home=$(cd "${HOME:-/}" 2>/dev/null && pwd -P) || home=/
    case "$tmp" in
      / | "$home") exit 1 ;;
    esac
    case "$home/" in
      "$tmp"/*) exit 1 ;;
    esac
    cd "$1" 2>/dev/null || exit 1
    top=$("$real_git" rev-parse --show-toplevel 2>/dev/null) || exit 1
    top=$(cd "$top" 2>/dev/null && pwd -P) || exit 1
    case "$top/" in
      "$tmp"/*) exit 0 ;;
    esac
    exit 1
  )
}

# True when the pathspec $1 of a `restore` stands for the whole working tree
# (KT-1753): `.` in any spelling (`./`, `./.`), the top-level pathspecs (`:/`,
# `:/.`, `:/*`, `:(top)`), a quoted `*`, or a path that resolves to the top
# level itself (its own path, `..` from a sub-directory). $kronn_top is the
# top level of the repository git runs in (empty when it is not one).
kronn_is_whole_tree() {
  case "$1" in
    . | ./ | ./. | .// | \* | :/ | :/. | :/\* | ':(top)' | ':(top).' | ':(top)*' | ':(top)/' | ':(top)/*')
      return 0
      ;;
    -* | :*) return 1 ;;
  esac
  [ -n "$kronn_top" ] || return 1
  case "$1" in
    /*) kronn_target=$1 ;;
    *) kronn_target=$kronn_dir/$1 ;;
  esac
  kronn_resolved=$(cd "$kronn_target" 2>/dev/null && pwd -P) || return 1
  [ "$kronn_resolved" = "$kronn_top" ]
}

# Succeeds when the arguments after the subcommand ($1, then the command line)
# are the destructive form of that subcommand (KT-1753): `restore` of the whole
# tree into the working tree, `clean` that is not a dry run, `branch` forced
# (-f, -D, and the forced rename or copy -M, -C).
kronn_destructive() {
  kronn_want=$1
  shift
  # Drop git's global options and the subcommand itself.
  while [ $# -gt 0 ]; do
    if [ "$1" = "$kronn_want" ]; then
      shift
      break
    fi
    shift
  done
  kronn_whole=0
  kronn_staged=0
  kronn_worktree=0
  kronn_dry=0
  kronn_force=0
  kronn_skip=0
  kronn_exclude=0
  kronn_positive=0
  kronn_top=
  if [ "$kronn_want" = restore ]; then
    kronn_top=$(cd "$kronn_dir" 2>/dev/null && "$real_git" rev-parse --show-toplevel 2>/dev/null) || kronn_top=
    [ -z "$kronn_top" ] || kronn_top=$(cd "$kronn_top" 2>/dev/null && pwd -P) || kronn_top=
  fi
  for kronn_a in "$@"; do
    # The value of an option that takes one (`--sort -refname`) is not an option.
    if [ "$kronn_skip" = 1 ]; then
      kronn_skip=0
      continue
    fi
    case "$kronn_want:$kronn_a" in
      restore:--staged | restore:-S) kronn_staged=1 ;;
      restore:--worktree | restore:-W) kronn_worktree=1 ;;
      # Pathspecs read from a file cannot be judged here: treated as the
      # whole tree. `-s <tree>` takes a value that is not a pathspec.
      restore:--pathspec-from-file*) kronn_whole=1 ;;
      restore:-s | restore:--source) kronn_skip=1 ;;
      restore:-*) ;;
      # An exclude-only restore (`:!x`, `:^x`, `:(exclude)x`, and the same
      # behind the top-level magic: `:/!x`, `:/^x`) covers the rest of the tree.
      restore:':!'* | restore:':^'* | restore:':/!'* | restore:':/^'* | \
        restore:':('*exclude*')'*)
        kronn_exclude=1
        ;;
      restore:*)
        kronn_positive=1
        kronn_is_whole_tree "$kronn_a" && kronn_whole=1
        ;;
      clean:--dry-run) kronn_dry=1 ;;
      clean:--*) ;;
      clean:-*n*) kronn_dry=1 ;;
      branch:--force) kronn_force=1 ;;
      branch:--sort | branch:--format | branch:--contains | branch:--no-contains | \
        branch:--merged | branch:--no-merged | branch:--points-at | branch:--set-upstream-to | \
        branch:-u | branch:--column)
        kronn_skip=1
        ;;
      branch:--*) ;;
      branch:-*f* | branch:-*D* | branch:-*M* | branch:-*C*) kronn_force=1 ;;
    esac
  done
  if [ "$kronn_exclude" = 1 ] && [ "$kronn_positive" = 0 ]; then
    kronn_whole=1
  fi
  case "$kronn_want" in
    restore)
      [ "$kronn_whole" = 1 ] && { [ "$kronn_staged" = 0 ] || [ "$kronn_worktree" = 1 ]; }
      ;;
    clean) [ "$kronn_dry" = 0 ] ;;
    branch) [ "$kronn_force" = 1 ] ;;
    *) return 1 ;;
  esac
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
  restore | clean | branch)
    if kronn_destructive "$kronn_sub" "$@"; then
      guarded=$kronn_sub
    fi
    ;;
esac

if [ -n "$guarded" ] && [ "${KRONN_MAIN_CHECKOUT_GIT_CONFIRMED-}" != 1 ]; then
  if kronn_in_main_checkout "$kronn_dir" && ! kronn_in_scratch_repo "$kronn_dir"; then
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
