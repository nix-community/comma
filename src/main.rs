mod cache;
mod cli;
mod confirm;
mod index;
mod install;
mod lookup;
mod nixpkgs;
mod run;
mod shell;

use std::{
    env, io,
    path::Path,
    process::{self, ExitCode},
};

use cache::Cache;
use clap::{CommandFactory, Parser};
use clap_complete::{generate, Generator};
use cli::{ManArgs, Opt, SubCmds};
use confirm::{confirmer, EXIT_NOT_RUN};
use index::{index_database, locate};
use install::Installer;
use log::{debug, error};
use lookup::{choose_entry, find_command, find_entry, Found};
use nixpkgs::Nixpkgs;
use run::{
    all_outputs, exec, get_command_path_from_cache, run_command_from_cache,
    run_command_or_open_shell,
};

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
fn open_shell(args: &Opt, mut cache: Option<Cache>, nixpkgs: &Nixpkgs) -> ExitCode {
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
    let shell_cmd = shell::select_shell_from_pid(process::id()).unwrap_or("bash".into());
    let run_cmd = run_command_or_open_shell(nixpkgs, &derivations, &shell_cmd, &[]);

    exec(run_cmd, cache)
}

fn main() -> ExitCode {
    env_logger::init();

    let args = Opt::parse();

    if args.mangen {
        let man = clap_mangen::Man::new(Opt::command());

        if let Err(err) = man.render(&mut std::io::stdout()) {
            panic!("{}", err)
        } else {
            return ExitCode::SUCCESS;
        }
    }

    if let Some(shell) = args.print_completions {
        let mut cmd = Opt::command();
        eprintln!("Generating completion file for {shell}...");
        let bin_name = std::env::args()
            .next()
            .and_then(|p| {
                Path::new(&p)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "comma".into());
        print_completions(shell, &mut cmd, &bin_name);
        return ExitCode::SUCCESS;
    }

    let mut cache = if args.cache_level == 0 {
        None
    } else {
        match Cache::new() {
            Err(e) => {
                error!("failed to initialize cache, disabling related functionality: {e}");
                None
            }
            Ok(x) => Some(x),
        }
    };

    if args.empty_cache {
        if let Some(ref mut cache) = cache {
            cache.empty();
        }
    }

    if args.print_cache {
        return if let Some(ref cache) = cache {
            print_cache(cache);
            ExitCode::SUCCESS
        } else {
            eprintln!("The cache is disabled");
            ExitCode::FAILURE
        };
    }

    if args.cmd.is_empty() && args.subcmds.is_none() {
        return if args.empty_cache {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    let nixpkgs = Nixpkgs::from_args(args.nixpkgs_flake.as_deref());

    // --shell is a mode of its own: it takes several commands, where
    // everything below is about a single one
    if args.shell {
        return open_shell(&args, cache, &nixpkgs);
    }

    let (command, trail) = if let Some(SubCmds::Man(ManArgs { ref cmd })) = args.subcmds {
        (&cmd[0], &cmd[1..])
    } else {
        (&args.cmd[0], &args.cmd[1..])
    };

    if args.delete_entry {
        if let Some(ref mut cache) = cache {
            cache.delete(command);
        }
    }

    if args.print_packages {
        match index_database(command) {
            Some(derivations) => {
                println!(
                    "Packages that contain /bin/{command}:\n{}",
                    derivations
                        .iter()
                        .map(|a| format!("- {a}"))
                        .collect::<Box<[String]>>()
                        .join("\n")
                );

                return ExitCode::SUCCESS;
            }
            None => return ExitCode::FAILURE,
        }
    }

    let Some((entry, picked)) = find_entry(cache.as_mut(), command, &args.picker) else {
        return ExitCode::FAILURE;
    };

    // Whether evaluated paths for this command are read from and written to
    // the cache. Commands listed in COMMA_NO_CACHE_PATH (comma separated) are
    // always evaluated again, but their package choice is still cached
    let no_cache_path = env::var("COMMA_NO_CACHE_PATH").unwrap_or_default();
    let cache_paths =
        args.cache_level >= 2 && !no_cache_path.split(',').any(|c| c.trim() == command);
    if !cache_paths {
        debug!(
            "cache_level={}, COMMA_NO_CACHE_PATH={no_cache_path:?}: not using cached paths for '{command}'",
            args.cache_level
        );
    }

    // The cache used for path lookups; the choice is still kept in `cache`
    let path_cache = cache.as_mut().filter(|_| cache_paths);

    if args.install {
        let install_cmd = Installer::detect().command(&nixpkgs, &entry.derivation);

        exec(install_cmd, cache)
    } else if args.print_path {
        let Some(path) = get_command_path_from_cache(path_cache, &entry, &nixpkgs, command) else {
            return ExitCode::FAILURE;
        };
        println!("{path}");

        ExitCode::SUCCESS
    } else if args.subcmds.is_some() {
        // Open manpage via
        // nix shell nixpkgs#drvName --command man commandName
        let run_cmd = run_command_or_open_shell(
            &nixpkgs,
            &[&all_outputs(&entry.derivation)],
            "man",
            &[command],
        );

        exec(run_cmd, cache)
    } else {
        // Ask before the package is built or downloaded. Choosing it in the
        // picker was already the confirmation
        if args.ask && !picked && !confirmer(command, &entry.derivation) {
            return ExitCode::from(EXIT_NOT_RUN);
        }

        let Some(run_cmd) = run_command_from_cache(path_cache, &entry, &nixpkgs, command, trail)
        else {
            return ExitCode::FAILURE;
        };

        exec(run_cmd, cache)
    }
}

fn print_cache(cache: &Cache) {
    for (command, entry) in cache.entries() {
        println!("{command} -> {}", entry.derivation);

        let mut paths: Vec<_> = entry.paths.iter().collect();
        paths.sort();
        let width = paths
            .iter()
            .map(|(source, _)| source.len())
            .max()
            .unwrap_or(0);
        for (source, path) in paths {
            let note = if Path::new(path).exists() {
                ""
            } else {
                " (garbage collected)"
            };
            println!("  {source:width$}  {path}{note}");
        }
    }
}

fn print_completions<G: Generator>(generator: G, cmd: &mut clap::Command, bin_name: &str) {
    generate(generator, cmd, bin_name, &mut io::stdout());
}

#[cfg(test)]
mod tests {
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
