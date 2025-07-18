function __fish_command_not_found_handler --on-event fish_command_not_found
    comma --ask "$@"
    return $argv
end
