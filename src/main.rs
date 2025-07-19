mod cache;
mod index;
mod shell;

use std::{
    env,
    io::{self, Write},
    os::unix::prelude::CommandExt,
    path::Path,
    process::{self, Command, ExitCode, Stdio},
};

use cache::{Cache, CacheEntry};
use clap::crate_version;
use clap::Parser;
use log::{debug, trace};

fn pick(picker: &str, derivations: &[String]) -> Option<String> {
    let mut picker_process = Command::new(picker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| panic!("failed to execute {picker}: {err}"));

    let picker_stdin = picker_process.stdin.as_mut().unwrap();

    picker_stdin
        .write_all(derivations.join("\n").as_bytes())
        .expect("failure to write stdin");

    let output = picker_process.wait_with_output().unwrap().stdout;

    if output.is_empty() {
        return None;
    }
    Some(
        std::str::from_utf8(&output)
            .unwrap_or_else(|e| panic!("{picker} outputted invalid UTF-8: {e}"))
            .trim()
            .to_owned(),
    )
}

fn index_database(command: &str) -> Option<Box<[String]>> {
    index::check_database_updated();

    let nix_locate_output = Command::new("nix-locate")
        .args(["--minimal", "--at-root", "--whole-name"])
        .arg(format!("/bin/{command}"))
        .output()
        .expect("failed to execute nix-locate");

    if !nix_locate_output.status.success() {
        match std::str::from_utf8(&nix_locate_output.stderr) {
            Ok(stderr) => eprintln!("nix-locate failed with: {stderr}"),
            Err(_) => eprintln!("nix-locate failed"),
        }
        return None;
    }

    let attrs = nix_locate_output.stdout;

    if attrs.is_empty() {
        eprintln!("No executable `{command}` found in nix-index database.");
        return None;
    }

    Some(
        std::str::from_utf8(&attrs)
            .expect("fail")
            .trim()
            .split('\n')
            .map(|s| s.to_owned())
            .collect(),
    )
}

fn index_database_pick(command: &str, picker: &str) -> Option<String> {
    let attrs = index_database(command)?;

    if attrs.len() > 1 {
        pick(picker, &attrs)
    } else {
        attrs.first().map(|s| s.trim().to_owned())
    }
}

fn run_command_or_open_shell(
    use_channel: bool,
    choice: &str,
    command: &str,
    trail: &[String],
    nixpkgs_flake: &str,
) -> Command {
    let mut run_cmd = Command::new("nix");

    run_cmd.args([
        "--extra-experimental-features",
        "nix-command flakes",
        "shell",
    ]);

    if use_channel {
        run_cmd.args(["-f", "<nixpkgs>", choice]);
    } else {
        run_cmd.args([format!("{nixpkgs_flake}#{choice}")]);
    }

    if !command.is_empty() {
        run_cmd.args(["--command", command]);
        if !trail.is_empty() {
            run_cmd.args(trail);
        }
    };

    trace!("run nix command arguments: {run_cmd:?}");

    run_cmd
}

fn get_command_path(use_channel: bool, choice: &str, command: &str, nixpkgs_flake: &str) -> String {
    let mut run_cmd = Command::new("nix");

    run_cmd.args([
        "--extra-experimental-features",
        "nix-command flakes",
        "build",
        "--print-out-paths",
        "--no-link",
    ]);

    if use_channel {
        run_cmd.args(["-f", "<nixpkgs>", choice]);
    } else {
        run_cmd.args([format!("{nixpkgs_flake}#{choice}")]);
    }

    let result = run_cmd
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| panic!("failed to execute nix: {err}"));

    // It is safe to assume that only one path will be printed because
    // nix-locate appends the output to the derivation name
    // (e.g., firefox.out instead of firefox)
    let output = result.wait_with_output().unwrap().stdout;
    let base_path = std::str::from_utf8(&output)
        .unwrap_or_else(|err| panic!("nix outputted invalid UTF-8: {err}"))
        .trim();

    // It is safe to assume that command is in $out/bin/{command} from
    // the derivation, since this was already filtered by nix-locate
    format!("{base_path}/bin/{command}")
}

fn get_command_path_from_cache(
    cache: &mut Option<Cache>,
    entry: &CacheEntry,
    use_channel: bool,
    command: &str,
    nixpkgs_flake: &str,
) -> String {
    match &entry.path {
        // If we have the path in the cache and it is not garbage collected
        // (so the path still exists), it should be safe to use it directly
        Some(path) if Path::new(&path).exists() => {
            debug!("found path from cache for command '{command}': {path}");
            path.to_owned()
        }
        // Otherwise, we need to find the command path
        _ => match cache {
            Some(ref mut cache) => {
                let path = get_command_path(use_channel, &entry.derivation, command, nixpkgs_flake);
                debug!("found path from nix for command '{command}': {path}");

                let entry = CacheEntry {
                    path: Some(path.clone()),
                    ..entry.clone()
                };
                cache.update(command, entry);

                path
            }

            None => {
                let path = get_command_path(use_channel, &entry.derivation, command, nixpkgs_flake);
                debug!("found path from nix for command '{command}': {path}");

                path
            }
        },
    }
}

fn run_command_from_cache(
    cache: &mut Option<Cache>,
    entry: &CacheEntry,
    use_channel: bool,
    command: &str,
    trail: &[String],
    nixpkgs_flake: &str,
) -> Command {
    let path = get_command_path_from_cache(cache, entry, use_channel, command, nixpkgs_flake);

    let mut run_cmd = Command::new(path);
    if !trail.is_empty() {
        run_cmd.args(trail);
    }

    trace!("run command from cache arguments: {run_cmd:?}");

    run_cmd
}

fn confirmer(run_cmd: &Command) -> bool {
    loop {
        print!("Run '{}'? [Y/n]: ", run_cmd.get_program().display());
        io::stdout().flush().unwrap();

        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();

        match input.trim().to_lowercase().as_str() {
            "y" | "yes" | "" => return true,
            "n" | "no" => return false,
            _ => {
                println!("Please enter 'y' or 'n'.");
            }
        }
    }
}

fn main() -> ExitCode {
    env_logger::init();

    let args = Opt::parse();

    let mut cache = if args.cache_level == 0 {
        None
    } else {
        match Cache::new() {
            Err(e) => {
                eprintln!("failed to initialize cache, disabling related functionality: {e}");
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

    if args.cmd.is_empty() {
        return if args.empty_cache {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    let command = &args.cmd[0];
    let trail = &args.cmd[1..];

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

    let entry = match cache {
        Some(ref mut cache) => cache.query(command).or_else(|| {
            index_database_pick(command, &args.picker).map(|derivation| {
                let entry = CacheEntry {
                    derivation,
                    path: None,
                };
                cache.update(command, entry.clone());
                entry
            })
        }),
        None => index_database_pick(command, &args.picker).map(|derivation| CacheEntry {
            derivation,
            path: None,
        }),
    };

    let entry = match entry {
        Some(d) if args.cache_level >= 2 => d,
        Some(d) => {
            debug!("cache_level={}, ignoring path from cache", args.cache_level);
            CacheEntry {
                derivation: d.derivation.clone(),
                path: None,
            }
        }
        None => return ExitCode::FAILURE,
    };

    let basename = entry.derivation.rsplit('.').next_back().unwrap();

    let use_channel = env::var("NIX_PATH")
        .unwrap_or_default()
        .contains("nixpkgs=");

    if args.install {
        let _ = Command::new("nix-env")
            .args(["-f", "<nixpkgs>", "-iA", basename])
            .exec();
    } else if args.shell {
        // TODO: use cache here, but this is tricky since it actually depends in `nix-shell`
        let shell_cmd = shell::select_shell_from_pid(process::id()).unwrap_or("bash".into());
        let _ = run_command_or_open_shell(
            use_channel,
            &entry.derivation,
            &shell_cmd,
            &[],
            &args.nixpkgs_flake,
        )
        .exec();
    } else if args.print_path {
        let path = get_command_path_from_cache(
            &mut cache,
            &entry,
            use_channel,
            command,
            &args.nixpkgs_flake,
        );
        println!("{path}");
    } else {
        let mut run_cmd = run_command_from_cache(
            &mut cache,
            &entry,
            use_channel,
            command,
            trail,
            &args.nixpkgs_flake,
        );

        // Drop cache before calling exec() to make sure that
        // the cache file is written
        drop(cache);
        let answer = if args.ask { confirmer(&run_cmd) } else { true };
        if answer {
            let _ = run_cmd.exec();
        }
    }

    ExitCode::SUCCESS
}

/// Runs programs without installing them
#[derive(Parser)]
#[clap(version = crate_version!(), trailing_var_arg = true)]
struct Opt {
    /// Install the derivation containing the executable
    #[clap(short, long)]
    install: bool,

    /// Open a shell containing the derivation containing the executable
    #[clap(short, long)]
    shell: bool,

    /// Picker to use
    #[clap(short = 'P', long, env = "COMMA_PICKER", default_value = "fzy")]
    picker: String,

    /// Nixpkgs flake to use
    #[clap(
        short = 'F',
        long,
        env = "COMMA_NIXPKGS_FLAKE",
        default_value = "nixpkgs"
    )]
    nixpkgs_flake: String,

    /// Ask to confirm the program that will be run.
    #[clap(short, long, env = "COMMA_ASK_TO_CONFIRM")]
    ask: bool,

    /// Print the package containing the executable
    #[clap(short = 'p', long = "print-packages")]
    print_packages: bool,

    /// Print the absolute path to the executable in the nix store
    #[clap(short = 'x', long = "print-path")]
    print_path: bool,

    /// Configure the cache level. 0 disables the cache, 1 enables cache for
    /// choices, 2 also caches path evaluations
    #[clap(long = "cache-level", env = "COMMA_CACHING", default_value_t = 2)]
    cache_level: u8,

    /// Empty the cache
    #[clap(short, long = "empty-cache")]
    empty_cache: bool,

    /// Overwrite the cache entry for the specified command. This is achieved
    /// by first deleting it from the cache, then running comma as normal
    #[clap(short, long = "delete-entry")]
    delete_entry: bool,

    /// Command to run
    #[clap(required_unless_present_any = ["empty_cache"], name = "cmd")]
    cmd: Vec<String>,
}
