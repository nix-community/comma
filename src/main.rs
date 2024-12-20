mod cache;
mod index;
mod shell;

use std::{
    env,
    io::Write,
    os::unix::prelude::CommandExt,
    process::{self, Command, ExitCode, Stdio},
};

use cache::Cache;
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
    derivations: &[String],
    command: &str,
    trail: &[String],
    nixpkgs_flake: &str,
) {
    let mut run_cmd = Command::new("nix");

    run_cmd.args([
        "--extra-experimental-features",
        "nix-command flakes",
        "shell",
    ]);

    if use_channel {
        run_cmd.args(["-f", "<nixpkgs>"]);
        run_cmd.args(derivations);
    } else {
        for derivation in derivations {
            run_cmd.args([format!("{nixpkgs_flake}#{derivation}")]);
        }
    }

    if !command.is_empty() {
        run_cmd.args(["--command", command]);
        if !trail.is_empty() {
            run_cmd.args(trail);
        }
    };

    run_cmd.exec();
}

fn main() -> ExitCode {
    let args = Opt::parse();

    let mut cache = Cache::new()
        .map_err(|e| {
            eprintln!("failed to initialize cache, disabling related functionality: {e}");
        })
        .ok();

    if args.update {
        eprintln!("\"comma --update\" has been deprecated. either obtain a prebuilt database from https://github.com/Mic92/nix-index-database or use \"nix run 'nixpkgs#nix-index' --extra-experimental-features 'nix-command flakes'\"");
        index::update_database();
    }

    if args.empty_cache {
        if let Some(ref mut cache) = cache {
            cache.empty();
        }
    }

    let use_channel = env::var("NIX_PATH").is_ok_and(|p| p.contains("nixpkgs="));

    if !args.shell.is_empty() {
        return open_shell(
            args.shell,
            args.delete_entry,
            &args.picker,
            cache,
            use_channel,
            args.nixpkgs_flake,
        );
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

    let Some(derivation) = find_program(command, &args.picker, cache.as_mut(), args.delete_entry)
    else {
        eprint!("Error");
        return ExitCode::FAILURE;
    };
    // Explicitly drop cache because exec doesn't call destructors
    drop(cache);
    let basename = derivation.rsplit('.').last().unwrap();

    if args.install {
        Command::new("nix-env")
            .args(["-f", "<nixpkgs>", "-iA", basename])
            .exec();
    } else if args.print_path {
        run_command_or_open_shell(
            use_channel,
            &[derivation],
            "sh",
            &[
                String::from("-c"),
                format!("printf '%s\n' \"$(realpath \"$(which {command})\")\""),
            ],
            &args.nixpkgs_flake,
        );
    } else {
        run_command_or_open_shell(
            use_channel,
            &[derivation],
            command,
            trail,
            &args.nixpkgs_flake,
        );
    }

    ExitCode::SUCCESS
}

fn open_shell(
    commands: Vec<String>,
    empty_cache: bool,
    picker: &str,
    mut cache: Option<Cache>,
    use_channel: bool,
    nixpkgs_flake: String,
) -> ExitCode {
    let shell_cmd = shell::select_shell_from_pid(process::id()).unwrap_or("bash".into());
    let mut programs = Vec::new();
    for prog in commands {
        let Some(derivation) = find_program(&prog, picker, cache.as_mut(), empty_cache) else {
            eprint!("Couldn't find program for {prog}");
            return ExitCode::FAILURE;
        };
        programs.push(derivation);
    }

    // Explicitly drop cache because exec doesn't call destructors
    drop(cache);

    run_command_or_open_shell(
        use_channel,
        &programs,
        &shell_cmd,
        &[],
        nixpkgs_flake.as_str(),
    );
    ExitCode::SUCCESS
}

/// Try to find and select a program from a command
fn find_program(
    command: &str,
    picker: &str,
    cache: Option<&mut Cache>,
    delete_entry: bool,
) -> Option<String> {
    cache.map_or_else(
        || index_database_pick(command, picker),
        |cache| {
            if delete_entry {
                cache.delete(command);
            }
            cache.query(command).or_else(|| {
                index_database_pick(command, picker).map(|derivation| {
                    cache.update(command, &derivation);
                    derivation
                })
            })
        },
    )
}

/// Runs programs without installing them
#[derive(Parser)]
#[clap(version = crate_version!(), trailing_var_arg = true)]
struct Opt {
    /// Install the derivation containing the executable
    #[clap(short, long)]
    install: bool,

    /// Open a shell with the derivations of the specified installables
    #[clap(short, long, conflicts_with_all(["cmd", "install"]), num_args(1..), value_name("cmd"))]
    shell: Vec<String>,

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
    #[clap(required_unless_present_any = ["update", "empty_cache", "shell"], name = "cmd")]
    cmd: Vec<String>,
}
