mod cache;
mod index;
mod shell;

use std::{
    env,
    error::Error,
    io::Write,
    os::unix::prelude::CommandExt,
    path::Path,
    process::{self, Command, ExitCode, Stdio},
};

use cache::{Cache,CacheEntry};
use clap::crate_version;
use clap::Parser;

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
        .args(["--top-level", "--minimal", "--at-root", "--whole-name"])
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

    run_cmd
}

fn get_command_path(
    use_channel: bool,
    choice: &str,
    command: &str,
    nixpkgs_flake: &str,
) -> Command {
    run_command_or_open_shell(
        use_channel,
        choice,
        "sh",
        &[
            String::from("-c"),
            format!("printf '%s\n' \"$(realpath \"$(which {command})\")\""),
        ],
        nixpkgs_flake,
    )
}

fn run_command_from_cache(
    cache: &mut Result<Cache, Box<dyn Error>>,
    use_channel: bool,
    choice: &str,
    command: &str,
    trail: &[String],
    nixpkgs_flake: &str,
) -> Command {
    match cache {
        Ok(ref mut cache) => {
            let mut nix_cmd = get_command_path(
                use_channel,
                choice,
                command,
                nixpkgs_flake,
            );
            let output = nix_cmd.stdout(Stdio::piped()).output().expect("failed to run nix");
            let path = String::from_utf8(output.stdout).expect("failed to decode UTF-8");
            let entry = CacheEntry {
                derivation: choice.to_string(),
                path: Some(path.trim().to_string()),
            };
            cache.update(command, entry);

            let mut run_cmd = Command::new(path.trim());
            // Need to set arg0 here to handle cases like busybox,
            // where the behavior of the program depends in the arg0
            run_cmd.arg0(command);
            if !trail.is_empty() {
                run_cmd.args(trail);
            }

            run_cmd
        }
        Err(_) => {
            run_command_or_open_shell(
                use_channel,
                choice,
                command,
                trail,
                nixpkgs_flake,
            )
        }
    }
}

fn main() -> ExitCode {
    let args = Opt::parse();

    let mut cache = Cache::new();
    if let Err(ref e) = cache {
        eprintln!("failed to initialize cache, disabling related functionality: {e}");
    }

    if args.update {
        eprintln!("\"comma --update\" has been deprecated. either obtain a prebuilt database from https://github.com/Mic92/nix-index-database or use \"nix run 'nixpkgs#nix-index' --extra-experimental-features 'nix-command flakes'\"");
        index::update_database();
    }

    if args.empty_cache {
        if let Ok(ref mut cache) = cache {
            cache.empty();
        }
    }

    // The command may not be given if `--update` was specified.
    if args.cmd.is_empty() {
        return if args.update || args.empty_cache {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    let command = &args.cmd[0];
    let trail = &args.cmd[1..];

    if args.delete_entry {
        if let Ok(ref mut cache) = cache {
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
        Ok(ref mut cache) => cache.query(command).or_else(|| {
            index_database_pick(command, &args.picker).map(|derivation| {
                let entry = CacheEntry {
                    derivation,
                    path: None,
                };
                cache.update(command, entry.clone());
                entry
            })
        }),
        Err(_) => index_database_pick(command, &args.picker).map(|derivation| {
            CacheEntry {
                derivation,
                path: None,
            }
        }),
    };

    let entry = match entry {
        Some(d) => d,
        None => return ExitCode::FAILURE,
    };

    let basename = entry.derivation.rsplit('.').last().unwrap();

    let use_channel = match env::var("NIX_PATH") {
        Ok(val) => val,
        Err(_) => String::new(),
    }
    .contains("nixpkgs=");

    if args.install {
        Command::new("nix-env")
            .args(["-f", "<nixpkgs>", "-iA", basename])
            .exec();
    } else if args.shell {
        let shell_cmd = shell::select_shell_from_pid(process::id()).unwrap_or("bash".into());
        run_command_or_open_shell(
            use_channel,
            &entry.derivation,
            &shell_cmd,
            &[],
            &args.nixpkgs_flake,
        ).exec();
    } else if args.print_path {
        get_command_path(
            use_channel,
            &entry.derivation,
            command,
            &args.nixpkgs_flake,
        ).exec();
    } else {
        match entry.path {
            Some(path) => {
                if Path::new(&path).exists() {
                    let mut run_cmd = Command::new(path);
                    run_cmd.arg0(command);
                    if !trail.is_empty() {
                        run_cmd.args(trail);
                    }
                    run_cmd.exec();
                } else {
                    let mut run_cmd = run_command_from_cache(
                        &mut cache,
                        use_channel,
                        &entry.derivation,
                        command,
                        trail,
                        &args.nixpkgs_flake,
                    );
                    // Drop cache before calling exec() to make sure that
                    // the cache file is written
                    drop(cache);
                    run_cmd.exec();
                }
            }
            None => {
                let mut run_cmd = run_command_from_cache(
                    &mut cache,
                    use_channel,
                    &entry.derivation,
                    command,
                    trail,
                    &args.nixpkgs_flake,
                );
                // Drop cache before calling exec() to make sure that
                // the cache file is written
                drop(cache);
                run_cmd.exec();
            }
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

    #[clap(short = 'P', long, env = "COMMA_PICKER", default_value = "fzy")]
    picker: String,

    #[clap(
        short = 'F',
        long,
        env = "COMMA_NIXPKGS_FLAKE",
        default_value = "nixpkgs"
    )]
    nixpkgs_flake: String,

    /// DEPRECATED Update nix-index database
    #[clap(short, long)]
    update: bool,

    /// Print the package containing the executable
    #[clap(short = 'p', long = "print-packages")]
    print_packages: bool,

    /// Print the absolute path to the executable in the nix store
    #[clap(short = 'x', long = "print-path")]
    print_path: bool,

    /// Empty the cache
    #[clap(short, long = "empty-cache")]
    empty_cache: bool,

    /// Overwrite the cache entry for the specified command. This is achieved by first deleting it
    /// from the cache, then running comma as normal.
    #[clap(short, long = "delete-entry")]
    delete_entry: bool,

    /// Command to run
    #[clap(required_unless_present_any = ["update", "empty_cache"], name = "cmd")]
    cmd: Vec<String>,
}
