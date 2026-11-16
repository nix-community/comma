mod cache;
mod cli;
mod confirm;
mod index;
mod install;
mod lookup;
mod nixpkgs;
mod run;
mod shell;
mod tools;

use std::{env, io, path::Path, process::ExitCode};

use cache::Cache;
use clap::{CommandFactory, Parser};
use clap_complete::{generate, Generator};
use cli::{ManArgs, Opt, SubCmds};
use confirm::{confirmer, EXIT_NOT_RUN};
use index::index_database;
use install::Installer;
use log::{debug, error};
use lookup::find_entry;
use nixpkgs::Nixpkgs;
use run::{
    all_outputs, exec, get_command_path_from_cache, run_command_from_cache,
    run_command_or_open_shell,
};
use shell::open_shell;

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
