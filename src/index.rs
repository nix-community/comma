use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
    sync::Once,
    time::{Duration, SystemTime},
};

/// Prints a warning if the nix-index database is non-existent
pub fn check_database_exists() -> Result<(), ()> {
    let database_file = get_database_file();
    if !database_file.exists() {
        eprintln!(
            "Warning: Nix-index database not found at {}.\n\
             If you use the nix-index-database module, set `programs.nix-index-database.comma.enable = true` instead of installing comma yourself.\n\
             Otherwise set NIX_INDEX_DATABASE to where the database is, obtain a prebuilt database from https://github.com/nix-community/nix-index-database or generate one with `nix run 'nixpkgs#nix-index' --extra-experimental-features 'nix-command flakes'`.",
            database_file.display()
        );
        return Err(());
    }
    Ok(())
}

/// Prints a warning if the nix-index database is out of date. Only the first
/// call does anything, so that the warning is printed once when several
/// commands are looked up
pub fn check_database_updated() {
    static CHECKED: Once = Once::new();

    CHECKED.call_once(|| {
        let database_file = get_database_file();
        if check_database_exists().is_ok() {
            if database_file.metadata().unwrap().permissions().readonly() {
                // If db is not writable, they are responsible for keeping it up to date
                // because if it's part of the nix store, the timestamp will always 1970-01-01.
                return;
            }
            if is_database_old(&database_file) {
                eprintln!(
                    "Warning: Nix-index database is older than 30 days, either obtain a prebuilt database from https://github.com/nix-community/nix-index-database or try updating with `nix run 'nixpkgs#nix-index' --extra-experimental-features 'nix-command flakes'`."
                );
            }
        }
    });
}

/// Get the location of the nix-index database file
fn get_database_file() -> PathBuf {
    if let Ok(db) = env::var("NIX_INDEX_DATABASE") {
        let path = PathBuf::from(db);
        if path.is_dir() {
            path.join("files")
        } else {
            path
        }
    } else {
        let base = xdg::BaseDirectories::with_prefix("nix-index").unwrap();
        let cache_dir = base.get_cache_home();
        cache_dir.join("files")
    }
}

/// Test whether the database is more than 30 days old
fn is_database_old(database_file: &Path) -> bool {
    let Ok(metadata) = database_file.metadata() else {
        return false;
    };

    let time_since_modified = metadata
        .modified()
        .unwrap_or_else(|_| SystemTime::now())
        .elapsed()
        .unwrap_or(Duration::new(0, 0));

    time_since_modified > Duration::from_hours(720) && !metadata.permissions().readonly()
}

/// The packages in the nix-index database that have `command`, or the
/// reason why there are none
pub fn locate(command: &str) -> Result<Box<[String]>, String> {
    let nix_locate_output = Command::new("nix-locate")
        .args(["--minimal", "--at-root", "--whole-name"])
        .arg(format!("/bin/{command}"))
        .output()
        .map_err(|err| format!("failed to execute nix-locate: {err}"))?;

    if !nix_locate_output.status.success() {
        return Err(match std::str::from_utf8(&nix_locate_output.stderr) {
            Ok(stderr) => format!("nix-locate failed with: {stderr}"),
            Err(_) => "nix-locate failed".to_owned(),
        });
    }

    let attrs = nix_locate_output.stdout;

    if attrs.is_empty() {
        return Err(format!(
            "No executable `{command}` found in nix-index database."
        ));
    }

    Ok(std::str::from_utf8(&attrs)
        .expect("fail")
        .trim()
        .split('\n')
        .map(std::borrow::ToOwned::to_owned)
        .collect())
}

pub fn index_database(command: &str) -> Option<Box<[String]>> {
    check_database_updated();

    locate(command).inspect_err(|err| eprintln!("{err}")).ok()
}
