#!/bin/sh

# bash command not found handler
command_not_found_handle() {
    comma --ask "$@"
    return $?
}

# zsh compatibility
command_not_found_handler () {
    command_not_found_handle "$@"
    return $?
}
