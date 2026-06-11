# Fish completions for notion-ctl (notion-river control client).
# Install to ~/.config/fish/completions/ or /usr/share/fish/vendor_completions.d/

# True when completing argument N (1-based) of subcommand CMD.
function __notion_ctl_arg -a cmd n
    set -l toks (commandline -opc)
    test (count $toks) -eq (math $n + 1); and test "$toks[2]" = "$cmd"
end

function __notion_ctl_workspaces
    notion-ctl list-workspaces 2>/dev/null | string match -arg '"name":"([^"]*)"'
end

function __notion_ctl_outputs
    notion-ctl list-workspaces 2>/dev/null | string match -arg '"output":"([^"]*)"' | sort -u
end

function __notion_ctl_app_ids
    notion-ctl list-windows 2>/dev/null | string match -arg '"app_id":"([^"]*)"' | sort -u
end

function __notion_ctl_bound_app_ids
    set -l f ~/.config/notion-river/bindings.json
    if type -q jq; and test -r $f
        jq -r '.bindings | keys[]' $f 2>/dev/null
    else
        __notion_ctl_app_ids
    end
end

function __notion_ctl_window_ids
    if type -q jq
        notion-ctl list-windows 2>/dev/null | jq -r '.[] | "\(.id)\t[\(.workspace // "float")] \(.title) (\(.app_id))"' 2>/dev/null
    else
        notion-ctl list-windows 2>/dev/null | string match -arg '"id":([0-9]+)'
    end
end

function __notion_ctl_identifiers
    if type -q jq
        notion-ctl list-windows 2>/dev/null | jq -r '.[] | "\(.identifier)\t[\(.workspace // "float")] \(.title) (\(.app_id))"' 2>/dev/null
    else
        notion-ctl list-windows 2>/dev/null | string match -arg '"identifier":"([^"]*)"'
    end
end

complete -c notion-ctl -f

# Subcommands
complete -c notion-ctl -n '__fish_use_subcommand' -a list-windows -d 'List open windows (JSON)'
complete -c notion-ctl -n '__fish_use_subcommand' -a list-workspaces -d 'List workspaces (JSON)'
complete -c notion-ctl -n '__fish_use_subcommand' -a subscribe-workspaces -d 'Stream workspace updates'
complete -c notion-ctl -n '__fish_use_subcommand' -a subscribe-workspace -d 'Stream updates for one workspace'
complete -c notion-ctl -n '__fish_use_subcommand' -a subscribe-output -d 'Stream updates for one output'
complete -c notion-ctl -n '__fish_use_subcommand' -a focus-window -d 'Focus window by id'
complete -c notion-ctl -n '__fish_use_subcommand' -a focus-window-by-identifier -d 'Focus window by stable identifier'
complete -c notion-ctl -n '__fish_use_subcommand' -a switch-workspace -d 'Switch to workspace'
complete -c notion-ctl -n '__fish_use_subcommand' -a bind -d 'Bind app to workspace frame'
complete -c notion-ctl -n '__fish_use_subcommand' -a unbind -d 'Remove app binding'
complete -c notion-ctl -n '__fish_use_subcommand' -a set-fixed-dimensions -d 'Force window dimensions for app'
complete -c notion-ctl -n '__fish_use_subcommand' -a save-monitors -d 'Save current monitor layout'
complete -c notion-ctl -n '__fish_use_subcommand' -a forget-monitors -d 'Forget saved monitor layout'

# Arguments
complete -c notion-ctl -n '__notion_ctl_arg subscribe-workspace 1' -a '(__notion_ctl_workspaces)'
complete -c notion-ctl -n '__notion_ctl_arg subscribe-output 1' -a '(__notion_ctl_outputs)'
complete -c notion-ctl -n '__notion_ctl_arg focus-window 1' -a '(__notion_ctl_window_ids)'
complete -c notion-ctl -n '__notion_ctl_arg focus-window-by-identifier 1' -a '(__notion_ctl_identifiers)'
complete -c notion-ctl -n '__notion_ctl_arg switch-workspace 1' -a '(__notion_ctl_workspaces)'
complete -c notion-ctl -n '__notion_ctl_arg bind 1' -a '(__notion_ctl_app_ids)'
complete -c notion-ctl -n '__notion_ctl_arg bind 2' -a '(__notion_ctl_workspaces)'
complete -c notion-ctl -n '__notion_ctl_arg bind 3' -a '0' -d 'Frame index'
complete -c notion-ctl -n '__notion_ctl_arg bind 4' -d 'WxH (e.g. 1920x1080)'
complete -c notion-ctl -n '__notion_ctl_arg unbind 1' -a '(__notion_ctl_bound_app_ids)'
complete -c notion-ctl -n '__notion_ctl_arg set-fixed-dimensions 1' -a '(__notion_ctl_app_ids)'
complete -c notion-ctl -n '__notion_ctl_arg set-fixed-dimensions 2' -a 'clear' -d 'Remove fixed dimensions'
