# Agent Interface starts zsh with ZDOTDIR naming this folder. Hand the rest of
# start-up back to the person's own files, then load the guard at the first
# prompt, after all of them have run.
builtin typeset -g __agent_interface_dir="${${(%):-%x}:A:h:h}"
builtin unset ZDOTDIR
if [[ -r "$HOME/.zshenv" ]]; then
  builtin source "$HOME/.zshenv"
fi
if [[ -o interactive ]]; then
  function __agent_interface_load {
    builtin autoload -Uz add-zsh-hook
    add-zsh-hook -d precmd __agent_interface_load
    builtin source "$__agent_interface_dir/guard.sh"
    builtin unset __agent_interface_dir
    builtin unfunction __agent_interface_load
  }
  builtin autoload -Uz add-zsh-hook
  add-zsh-hook precmd __agent_interface_load
else
  builtin unset __agent_interface_dir
fi
