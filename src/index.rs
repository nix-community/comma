use std::{
    os::unix::prelude::CommandExt,
    path::PathBuf,
    process::Command,
    time::{Duration, SystemTime},
};

/// Update the local nix-index database.
pub fn update_database() {
    println!("Updating nix-index database, takes around 5 minutes.");
    Command::new("nix-index").exec();
}

/// Prints a warning if the nix-index database is non-existent
pub fn check_database_exists() {
    let database_file = get_database_file();
    if !database_file.exists() {
        println!("Warning: Nix-index database does not exist, try updating with `comma --update`.");
    }
}

/// Prints a warning if the nix-index database is out of date.
pub fn check_database_updated() {
    let database_file = get_database_file();
    if is_database_old(database_file) {
        println!(
            "Warning: Nix-index database is older than 30 days, try updating with `comma --update`."
        );
    }
}

/// Get the location of the nix-index database file
fn get_database_file() -> PathBuf {
    let base = xdg::BaseDirectories::with_prefix("nix-index").unwrap();
    let cache_dir = base.get_cache_home();
    cache_dir.join("files")
}

/// Test whether the database is more than 30 days old
fn is_database_old(database_file: std::path::PathBuf) -> bool {
    let metadata = match database_file.metadata() {
        Ok(metadata) => metadata,
        Err(_) => return false,
    };

    let time_since_modified = metadata
        .modified()
        .unwrap_or_else(|_| SystemTime::now())
        .elapsed()
        .unwrap_or(Duration::new(0, 0));

    time_since_modified > Duration::from_secs(30 * 24 * 60 * 60)
        && !metadata.permissions().readonly()
}
