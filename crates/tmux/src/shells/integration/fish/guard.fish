# Agent Interface defines claude, codex and agy in a fish terminal it starts,
# through --init-command, after the person's own configuration. Typed by hand
# at a terminal, each one asks whether to start a Backbone agent instead.

function __agent_interface_guard
    set -l tool $argv[1]
    set -l runtime $argv[2]
    set -l label $argv[3]
    set -e argv[1..3]
    # An agent's own session, a script or pipe, or a machine without the
    # backbone program runs the tool as typed.
    if test -n "$BACKBONE_AGENT"; or not test -t 0; or not test -t 1; or not command -q backbone
        command $tool $argv
        return
    end
    read -l -P "You are starting $label outside Backbone: no skills, policies or messaging. Start it as a Backbone agent here instead? [Y/n] " answer
    or return 130
    if string match -qr '^[Nn]' -- $answer
        command $tool $argv
        return
    end
    set -l name (string replace -r '.*/' '' -- $PWD)
    read -l -P "Agent name [$name]: " given
    or return 130
    if test -n "$given"
        set name $given
    end
    if test -z "$name"
        return 1
    end
    command backbone agent start --dir $PWD --runtime $runtime -- $name
end

# A function the person defined under one of these names is theirs to keep.
if not functions -q claude
    function claude
        __agent_interface_guard claude claude Claude $argv
    end
end
if not functions -q codex
    function codex
        __agent_interface_guard codex codex Codex $argv
    end
end
if not functions -q agy
    function agy
        __agent_interface_guard agy antigravity Antigravity $argv
    end
end
