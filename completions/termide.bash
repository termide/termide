# Bash completion for termide.
#
# Install with either of:
#   eval "$(termide --completions bash)"          # in ~/.bashrc
#   termide --completions bash > /usr/share/bash-completion/completions/termide
#
# Option names are spelled out by hand; src/main.rs has a test that fails
# when a clap option is missing here.

# Ids of the detached instances `termide --list-instances` reports: the table's
# first column, minus its header. Prints nothing when there are none.
_termide_instances() {
  termide --list-instances 2>/dev/null | awk 'NR > 1 { print $1 }'
}

_termide() {
  local cur prev
  cur="${COMP_WORDS[COMP_CWORD]}"
  prev="${COMP_WORDS[COMP_CWORD-1]}"

  case "$prev" in
    --log-level)
      COMPREPLY=($(compgen -W "trace debug info warn error" -- "$cur"))
      return ;;
    --completions|--install-completions)
      COMPREPLY=($(compgen -W "bash zsh fish" -- "$cur"))
      return ;;
    --config)
      COMPREPLY=($(compgen -f -- "$cur"))
      return ;;
    --attach)
      # `--attach` takes an optional id; with no instance running the next
      # word can only be a file to open.
      COMPREPLY=($(compgen -W "$(_termide_instances)" -- "$cur"))
      [[ ${#COMPREPLY[@]} -eq 0 ]] && COMPREPLY=($(compgen -f -- "$cur"))
      return ;;
    --kill)
      COMPREPLY=($(compgen -W "$(_termide_instances)" -- "$cur"))
      return ;;
  esac

  if [[ "$cur" == -* ]]; then
    COMPREPLY=($(compgen -W "--log-level --no-lsp -r --restore --config --diagnostics \
      --detached --attach -f --force --kill --list-instances --completions --install-completions \
      --prompt --recall --agent --output --help --version" -- "$cur"))
  else
    COMPREPLY=($(compgen -f -- "$cur"))
  fi
}

complete -o filenames -F _termide termide
