use std::{
    env,
    os::unix::prelude::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime},
};

/// Update the local nix-index database.
pub fn update_database() {
    eprintln!("Updating nix-index database, takes around 5 minutes.");
    Command::new("nix-index").exec();
}

/// Prints a warning if the nix-index database is non-existent
pub fn check_database_exists() -> Result<(), ()> {
    let database_file = get_database_file();
    if !database_file.exists() {
        eprintln!("Warning: Nix-index database does not exist, either obtain a prebuilt database from https://github.com/Mic92/nix-index-database or try updating with `nix run 'nixpkgs#nix-index' --extra-experimental-features 'nix-command flakes'`.");
        return Err(());
    }
    Ok(())
}

/// Prints a warning if the nix-index database is out of date.
pub fn check_database_updated() {
    let database_file = get_database_file();
    if check_database_exists().is_ok() {
        if database_file.metadata().unwrap().permissions().readonly() {
            // If db is not writable, they are responsible for keeping it up to date
            // because if it's part of the nix store, the timestamp will always 1970-01-01.
            return;
        };
        if is_database_old(&database_file) {
            eprintln!(
                "Warning: Nix-index database is older than 30 days, either obtain a prebuilt database from https://github.com/Mic92/nix-index-database or try updating with `nix run 'nixpkgs#nix-index' --extra-experimental-features 'nix-command flakes'`."
            );
        }
    };
}

/// Get the location of the nix-index database file
fn get_database_file() -> PathBuf {
    match env::var("NIX_INDEX_DATABASE") {
        Ok(db) => PathBuf::from(db),
        Err(_) => {
            let base = xdg::BaseDirectories::with_prefix("nix-index").unwrap();
            let cache_dir = base.get_cache_home();
            cache_dir.join("files")
        }
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

    time_since_modified > Duration::from_secs(30 * 24 * 60 * 60)
        && !metadata.permissions().readonly()
}
