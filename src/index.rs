use std::{
    os::unix::prelude::CommandExt,
    process::Command,
    time::{Duration, SystemTime},
};

/// Update the local nix-index database.
pub fn update_database() {
    println!("Updating nix-index database, takes around 5 minutes.");
    Command::new("nix-index").exec();
}

/// Prints warnings if the nix-index database is non-existent or out of date.
pub fn check_database() {
    let base = xdg::BaseDirectories::with_prefix("nix-index").unwrap();
    let cache_dir = base.get_cache_home();
    let database_file = cache_dir.join("files");
    if !database_file.exists() {
        println!("Warning: Nix-index database does not exist, try updating with `--update`.");
    } else if is_database_old(database_file) {
        println!(
            "Warning: Nix-index database is older than 30 days, try updating with `--update`."
        );
    }
}

/// Test whether the database is more than 30 days old
fn is_database_old(database_file: std::path::PathBuf) -> bool {
    let modified = match database_file.metadata() {
        Ok(metadata) => metadata.modified().unwrap_or_else(|_| SystemTime::now()),
        Err(_) => return false,
    };
    let time_since_modified = SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::new(0, 0));
    if time_since_modified > Duration::from_secs(30 * 24 * 60 * 60) {
        return true;
    }
    false
}
