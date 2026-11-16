use std::{
    io,
    os::unix::prelude::CommandExt,
    path::Path,
    process::{Command, ExitCode, Stdio},
};

use log::{debug, error, trace};

use crate::{
    cache::{Cache, CacheEntry},
    nixpkgs::Nixpkgs,
};

/// `nix shell` with the packages `choices`, running `command` with the
/// arguments `trail` in it
pub fn run_command_or_open_shell(
    nixpkgs: &Nixpkgs,
    choices: &[&str],
    command: &str,
    trail: &[&str],
) -> Command {
    let mut run_cmd = Command::new("nix");

    run_cmd.args([
        "--extra-experimental-features",
        "nix-command flakes",
        "shell",
    ]);

    run_cmd.args(nixpkgs.installable_args(choices));

    if !command.is_empty() {
        run_cmd.args(["--command", command]);
        if !trail.is_empty() {
            run_cmd.args(trail);
        }
    }

    trace!("run nix command arguments: {run_cmd:?}");

    run_cmd
}

/// The installable for all outputs of the package of `derivation`, which
/// nix-locate gives with the output that has the command appended, like
/// `micro.out` or `bind.dnsutils`. The man page can be in another output
pub fn all_outputs(derivation: &str) -> String {
    let attr = derivation
        .rsplit_once('.')
        .map_or(derivation, |(attr, _output)| attr);
    format!("{attr}^*")
}

/// The exit statuses a shell gives for a command that wasn't found, and for
/// one that was found but couldn't be executed
pub const EXIT_NOT_FOUND: u8 = 127;
const EXIT_NOT_EXECUTABLE: u8 = 126;

/// The exit status for `program`, which couldn't be executed because of
/// `err`. These are the ones a shell gives, so that this can be told apart
/// from the program itself failing
fn exec_failure_status(program: &Path, err: &io::Error) -> u8 {
    // A program that exists is "not found" when its interpreter is missing
    // (the one after `#!`, or the ELF loader). A shell says for that, that
    // the program couldn't be executed. A name without a directory is
    // searched in PATH, so it really wasn't found
    let exists = program.is_absolute() && program.exists();

    if err.kind() == io::ErrorKind::NotFound && !exists {
        EXIT_NOT_FOUND
    } else {
        EXIT_NOT_EXECUTABLE
    }
}

/// Replace this process with `cmd`. Only returns if that failed. Destructors
/// don't run when it succeeds, so `cache` is taken and dropped first, which
/// writes the cache file
pub fn exec(mut cmd: Command, cache: Option<Cache>) -> ExitCode {
    drop(cache);

    let err = cmd.exec();
    eprintln!(
        "comma: failed to execute '{}': {err}",
        cmd.get_program().to_string_lossy()
    );
    ExitCode::from(exec_failure_status(Path::new(cmd.get_program()), &err))
}

fn get_command_path(nixpkgs: &Nixpkgs, choice: &str, command: &str) -> Option<String> {
    let mut run_cmd = Command::new("nix");

    run_cmd.args([
        "--extra-experimental-features",
        "nix-command flakes",
        "build",
        "--print-out-paths",
        "--no-link",
    ]);

    run_cmd.args(nixpkgs.installable_args(&[choice]));

    let result = run_cmd
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| panic!("failed to execute nix: {err}"));

    // It is safe to assume that only one path will be printed because
    // nix-locate appends the output to the derivation name
    // (e.g., firefox.out instead of firefox)
    let output = result.wait_with_output().unwrap();
    if !output.status.success() {
        error!("nix build failed for '{choice}'");
        return None;
    }
    let base_path = std::str::from_utf8(&output.stdout)
        .unwrap_or_else(|err| panic!("nix outputted invalid UTF-8: {err}"))
        .trim();

    // It is safe to assume that command is in $out/bin/{command} from
    // the derivation, since this was already filtered by nix-locate
    Some(format!("{base_path}/bin/{command}"))
}

/// Find the path of `command`. Paths are only read from and written to `cache`
/// when it is given
pub fn get_command_path_from_cache(
    cache: Option<&mut Cache>,
    entry: &CacheEntry,
    nixpkgs: &Nixpkgs,
    command: &str,
) -> Option<String> {
    let source = nixpkgs.cache_key();
    let source = source.as_str();

    let cached = cache.as_ref().and(entry.paths.get(source));

    match cached {
        // If we have the path for this nixpkgs source in the cache and it is
        // not garbage collected (so the path still exists), it should be safe
        // to use it directly
        Some(path) if Path::new(path).exists() => {
            debug!("found path from cache for command '{command}' using '{source}': {path}");
            Some(path.to_owned())
        }
        // Otherwise, we need to find the command path
        _ => {
            let path = get_command_path(nixpkgs, &entry.derivation, command)?;
            debug!("found path from nix for command '{command}' using '{source}': {path}");

            if let Some(cache) = cache {
                let mut entry = entry.clone();
                entry.set_path(source, path.clone());
                cache.update(command, entry);
            }

            Some(path)
        }
    }
}

pub fn run_command_from_cache(
    cache: Option<&mut Cache>,
    entry: &CacheEntry,
    nixpkgs: &Nixpkgs,
    command: &str,
    trail: &[String],
) -> Option<Command> {
    let path = get_command_path_from_cache(cache, entry, nixpkgs, command)?;

    let mut run_cmd = Command::new(path);
    if !trail.is_empty() {
        run_cmd.args(trail);
    }

    trace!("run command from cache arguments: {run_cmd:?}");

    Some(run_cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_failure_statuses() {
        let not_found = io::Error::from(io::ErrorKind::NotFound);

        // A name is searched in PATH, whatever is in the current directory
        assert_eq!(exec_failure_status(Path::new("src"), &not_found), 127);
        assert_eq!(
            exec_failure_status(Path::new("/no/such/program"), &not_found),
            127
        );
        // It exists, so what wasn't found is its interpreter
        assert_eq!(exec_failure_status(Path::new("/"), &not_found), 126);
    }

    #[test]
    fn all_outputs_of_package() {
        assert_eq!(all_outputs("micro.out"), "micro^*");
        // The command can be in an output other than `out`
        assert_eq!(all_outputs("bind.dnsutils"), "bind^*");
        // Only the output is replaced, not `.out` inside the attribute path
        assert_eq!(
            all_outputs("python3Packages.outcome.out"),
            "python3Packages.outcome^*"
        );
    }
}
