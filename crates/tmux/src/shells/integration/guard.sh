# Agent Interface defines claude, codex and agy in a terminal it starts, after
# the person's own start-up files. Typed by hand at a terminal, each one asks
# whether to start a Backbone agent instead. Read by bash and zsh.
# `${1+"$@"}` because bash before 4.4 calls an empty "$@" unbound under `set -u`.

function __agent_interface_guard {
  local tool=$1 runtime=$2 label=$3 answer name backbone
  shift 3
  backbone=$(command -v backbone 2>/dev/null)
  # An agent's own session, a script or pipe, or a machine without the
  # backbone program runs the tool as typed.
  if [ -n "${BACKBONE_AGENT-}" ] || [ ! -t 0 ] || [ ! -t 1 ] ||
    [ "${backbone#/}" = "$backbone" ]; then
    command "$tool" ${1+"$@"}
    return
  fi
  printf '%s' "You are starting $label outside Backbone: no skills, policies or messaging. Start it as a Backbone agent here instead? [Y/n] "
  IFS= read -r answer || { echo; return 130; }
  case $answer in
    [Nn]*)
      command "$tool" ${1+"$@"}
      return
      ;;
  esac
  name=${PWD##*/}
  printf 'Agent name [%s]: ' "$name"
  IFS= read -r answer || { echo; return 130; }
  if [ -n "$answer" ]; then
    name=$answer
  fi
  if [ -z "$name" ]; then
    return 1
  fi
  command backbone agent start --dir "$PWD" --runtime "$runtime" -- "$name"
}

# A function the person defined under one of these names is theirs to keep.
if ! typeset -f claude >/dev/null 2>&1; then
  function claude { __agent_interface_guard claude claude Claude ${1+"$@"}; }
fi
if ! typeset -f codex >/dev/null 2>&1; then
  function codex { __agent_interface_guard codex codex Codex ${1+"$@"}; }
fi
if ! typeset -f agy >/dev/null 2>&1; then
  function agy { __agent_interface_guard agy antigravity Antigravity ${1+"$@"}; }
fi
