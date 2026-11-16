use std::{
    error::Error,
    fs,
    process::{self, ExitCode},
};

use crate::{
    cache::Cache,
    cli::{ManArgs, Opt, SubCmds},
    index::locate,
    lookup::{choose_entry, find_command, Found},
    nixpkgs::Nixpkgs,
    run::{exec, run_command_or_open_shell},
};

type ResultDyn<T> = Result<T, Box<dyn Error>>;

const KNOWN_SHELLS: &[&str] = &[
    "ash",    //
    "bash",   //
    "elvish", //
    "fish",   //
    "nu",     //
    "pwsh",   //
    "tcsh",   //
    "zsh",    //
];

fn get_process_status_field(pid: u32, field: &str) -> ResultDyn<String> {
    let status_bytes =
        fs::read(format!("/proc/{pid:?}/status")).map_err(|_| format!("no such pid: {pid:?}"))?;
    let status_str = String::from_utf8(status_bytes)?;
    let status_str = status_str
        .split('\n')
        .find(|&x| x.starts_with(field))
        .ok_or_else(|| format!("error parsing /proc/{pid:?}/status"))?;
    let field_contents = status_str
        .strip_prefix(&format!("{field}:"))
        .ok_or_else(|| "bad parsing".to_string())?
        .trim()
        .to_owned();
    Ok(field_contents)
}

fn get_parent_pid(pid: u32) -> ResultDyn<u32> {
    Ok(get_process_status_field(pid, "PPid")?.parse::<u32>()?)
}

fn get_process_name(pid: u32) -> ResultDyn<String> {
    get_process_status_field(pid, "Name")
}

fn get_all_parents_pid(pid: u32) -> ResultDyn<Vec<u32>> {
    let mut res = Vec::<u32>::new();
    let mut pid = pid;
    loop {
        match get_parent_pid(pid) {
            Ok(parent_id) if parent_id != 0 => {
                res.push(parent_id);
                pid = parent_id;
            }
            Ok(_) => return Ok(res),
            Err(e) => return Err(e),
        }
    }
}

fn select_shell_from_pid(pid: u32) -> ResultDyn<String> {
    let parents = get_all_parents_pid(pid)?;
    let parents_names: Result<Vec<_>, _> =
        parents.iter().map(|&pid| get_process_name(pid)).collect();
    let shell = parents_names?
        .into_iter()
        .find(|x| KNOWN_SHELLS.contains(&x.as_str()));
    shell.ok_or("no shell found".into())
}

/// The commands whose packages go in the shell, each of them once: with
/// --shell every argument is a command
fn shell_commands(args: &Opt) -> Vec<&str> {
    let mut given: Vec<&str> = args.cmd.iter().map(String::as_str).collect();

    // `man` in front of the other commands was parsed as the subcommand
    if let Some(SubCmds::Man(ManArgs { cmd })) = &args.subcmds {
        given.push("man");
        given.extend(cmd.iter().map(String::as_str));
    }

    let mut commands = Vec::new();
    for command in given {
        if !commands.contains(&command) {
            commands.push(command);
        }
    }
    commands
}

/// The package that a command is run from in a shell with `packages`, when
/// that isn't the package `chosen` for it. The packages come first in PATH in
/// their order, so another one of the `providers`, the packages that have the
/// command, wins when it is before the chosen one.
///
/// That is the order since nix 2.20:
/// <https://github.com/nixos/nix/commit/f4454aac9f002c33b84bb52e0bba2a46ea6170cd>.
/// Before it, `nix shell` put every package in front of the ones before it,
/// so the last one won. With such a nix this is wrong about which package a
/// command is run from
fn shadowing_package<'a>(
    packages: &[&'a str],
    chosen: &str,
    providers: &[String],
) -> Option<&'a str> {
    packages
        .iter()
        .copied()
        .take_while(|package| *package != chosen)
        .find(|package| providers.iter().any(|provider| provider == package))
}

/// Warn about the `commands` that aren't run from the package `chosen` for
/// them in a shell with `packages`
fn warn_about_shadowed(commands: &[&str], found: &[Found], chosen: &[String], packages: &[&str]) {
    for ((command, found), chosen) in commands.iter().zip(found).zip(chosen) {
        // Nothing is before the first package
        if packages.first() == Some(&chosen.as_str()) {
            continue;
        }

        // The index wasn't looked at for a remembered choice. When that
        // doesn't work now, there is nothing to warn about
        let providers = match found {
            Found::InIndex(attrs) => attrs.clone(),
            Found::Remembered(_) => locate(command).unwrap_or_default(),
        };

        if let Some(package) = shadowing_package(packages, chosen, &providers) {
            eprintln!(
                "Warning: '{command}' in the shell is the one from '{package}', \
                 which comes before '{chosen}' in PATH."
            );
        }
    }
}

/// Open a shell that has the packages of all the commands given in `args`
pub fn open_shell(args: &Opt, mut cache: Option<Cache>, nixpkgs: &Nixpkgs) -> ExitCode {
    let commands = shell_commands(args);

    // All the commands are looked up before the package of any of them is
    // asked for, so that the commands that don't exist are reported right
    // away, all of them, and not after the pickers of the commands before
    // them. With --delete-entry the remembered choices aren't used
    let remembered = cache.as_ref().filter(|_| !args.delete_entry);
    let found: Vec<Option<Found>> = commands
        .iter()
        .map(|command| find_command(remembered, command))
        .collect();
    let Some(found) = found.into_iter().collect::<Option<Vec<Found>>>() else {
        return ExitCode::FAILURE;
    };

    // Only now, so that the choices aren't forgotten for nothing when a
    // command doesn't exist
    if args.delete_entry {
        if let Some(cache) = cache.as_mut() {
            for command in &commands {
                cache.delete(command);
            }
        }
    }

    let mut chosen: Vec<String> = Vec::new();
    for (command, found) in commands.iter().zip(&found) {
        // The picker only shows the packages. With a single command it is
        // clear what they are for
        if commands.len() > 1 && matches!(found, Found::InIndex(attrs) if attrs.len() > 1) {
            eprintln!("Choose the package for '{command}':");
        }
        let Some((entry, _)) = choose_entry(cache.as_mut(), command, found, &args.picker) else {
            return ExitCode::FAILURE;
        };
        chosen.push(entry.derivation);
    }

    // The same package may provide several of the commands
    let mut derivations: Vec<&str> = Vec::new();
    for package in &chosen {
        if !derivations.contains(&package.as_str()) {
            derivations.push(package);
        }
    }

    warn_about_shadowed(&commands, &found, &chosen, &derivations);

    // TODO: use cache here, but this is tricky since it actually depends in `nix-shell`
    let shell_cmd = select_shell_from_pid(process::id()).unwrap_or("bash".into());
    let run_cmd = run_command_or_open_shell(nixpkgs, &derivations, &shell_cmd, &[]);

    exec(run_cmd, cache)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn shell_commands_are_all_arguments() {
        let commands = |args: &[&str]| {
            let args = Opt::try_parse_from([&[","], args].concat()).unwrap();
            shell_commands(&args)
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };

        assert_eq!(commands(&["-s", "cowsay", "lolcat"]), ["cowsay", "lolcat"]);
        // --shell is a flag, so it can be combined with other short flags
        assert_eq!(commands(&["-sd", "cowsay"]), ["cowsay"]);
        // A command that is given twice is there once
        assert_eq!(
            commands(&["-s", "cowsay", "lolcat", "cowsay"]),
            ["cowsay", "lolcat"]
        );
        // `man` is a command like the others, also in front of them, where
        // it is parsed as the subcommand
        assert_eq!(commands(&["-s", "cowsay", "man"]), ["cowsay", "man"]);
        assert_eq!(
            commands(&["-s", "man", "cowsay", "lolcat"]),
            ["man", "cowsay", "lolcat"]
        );
    }

    #[test]
    fn shadowing_packages() {
        let packages = ["xz.bin", "busybox.out", "lolcat.out"];
        let xz = ["xz.bin".to_owned(), "busybox.out".to_owned()];
        let lolcat = ["lolcat.out".to_owned()];

        // xz.bin has xz too and is before busybox.out
        assert_eq!(
            shadowing_package(&packages, "busybox.out", &xz),
            Some("xz.bin")
        );
        // The chosen package is before the other one that has it
        assert_eq!(shadowing_package(&packages, "xz.bin", &xz), None);
        // No other package has it
        assert_eq!(shadowing_package(&packages, "lolcat.out", &lolcat), None);
    }
}
