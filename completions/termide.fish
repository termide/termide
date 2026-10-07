# Fish completion for termide.
#
# Install with:
#   termide --completions fish > ~/.config/fish/completions/termide.fish
# or system-wide into /usr/share/fish/vendor_completions.d/termide.fish.
#
# Option names are spelled out by hand; src/main.rs has a test that fails
# when a clap option is missing here.

# Detached instances from the `--list-instances` table, header dropped: the id,
# then the project path as the description fish shows next to it.
function __termide_instances
    termide --list-instances 2>/dev/null | awk 'NR > 1 { print $1 "\t" $5 }'
end

complete -c termide -l log-level -d 'Minimum log level' -x -a 'trace debug info warn error'
complete -c termide -l no-lsp -d 'Disable LSP support'
complete -c termide -s r -l restore -d 'Reopen the projects of the last run'
complete -c termide -l config -d 'Path to config file' -r -F
complete -c termide -l diagnostics -d 'Run pre-flight diagnostics and exit'
complete -c termide -l detached -d 'Start a detached instance and print its id'
# `-r` so that fish completes the word after `--attach`; the id is optional to
# termide itself, so files stay on offer as well.
complete -c termide -l attach -d 'Attach to a detached instance, the most recent without an id' -r -a '(__termide_instances)'
complete -c termide -s f -l force -d 'With --attach, take the instance over from the attached client'
complete -c termide -l kill -d 'End a detached instance and exit' -x -a '(__termide_instances)'
complete -c termide -l list-instances -d 'List detached instances and exit'
complete -c termide -l completions -d 'Print a shell completion script and exit' -x -a 'bash zsh fish'
complete -c termide -l install-completions -d 'Install the completion script for a shell, $SHELL by default' -x -a 'bash zsh fish'
complete -c termide -l prompt -d 'Run one agent task without the UI and print the answer' -x
complete -c termide -l recall -d 'Search earlier agent sessions, git history and code and print the results' -x
complete -c termide -l agent -d 'Agent definition the --prompt run uses' -x
complete -c termide -l output -d 'Headless output format' -x -a 'text json stream-json'
complete -c termide -s h -l help -d 'Print help'
complete -c termide -s V -l version -d 'Print version'
