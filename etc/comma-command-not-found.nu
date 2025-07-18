$env.config.hooks.command_not_found = {
  |command_name|
  print (comma --ask $command_name | str trim)
}
