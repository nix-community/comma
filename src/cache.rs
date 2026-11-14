use std::{
    collections::HashMap,
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use bitcode::{Decode, Encode};
use log::{debug, error};

#[derive(Encode, Decode)]
struct CacheData(HashMap<String, CacheEntry>);

#[derive(Encode, Decode, Clone, Debug)]
pub struct CacheEntry {
    pub derivation: String,
    /// Evaluated command paths, keyed by the nixpkgs source (flake ref, or
    /// `<nixpkgs>` with the NIX_PATH it was resolved through) they were built
    /// from
    pub paths: HashMap<String, String>,
}

/// The path with the home directory replaced by `~`, so that messages
/// shown to the user don't expose their home directory
fn display_path(path: &Path) -> String {
    env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .and_then(|home| path.strip_prefix(home).ok())
        .map_or_else(
            || path.display().to_string(),
            |rest| format!("~/{}", rest.display()),
        )
}

pub struct Cache {
    path: PathBuf,
    data: CacheData,
    update: bool,
}

impl Cache {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let path = xdg::BaseDirectories::new()?.place_state_file("comma/choices")?;

        debug!("creating new cache instance for path: {}", path.display());

        Ok(Self {
            data: if path.exists() {
                let bytes = fs::read(&path)?;
                bitcode::decode(&bytes).unwrap_or_else(|e| {
                    // Shown by default, since the user's package choices are lost
                    eprintln!(
                        "comma: discarding the cache at '{}' because it could not \
                         be read: {e}\n\
                         comma: the cache format may have changed between comma \
                         versions. Remembered package choices and cached paths are \
                         lost, so you will be asked to pick packages again.",
                        display_path(&path)
                    );
                    CacheData(HashMap::new())
                })
            } else {
                CacheData(HashMap::new())
            },
            path,
            update: false,
        })
    }

    pub fn query(&self, command: &str) -> Option<CacheEntry> {
        debug!("querying cache entry for command '{command}'");
        self.data.0.get(command).cloned()
    }

    /// All cache entries, sorted by command name
    pub fn entries(&self) -> Vec<(&String, &CacheEntry)> {
        let mut entries: Vec<_> = self.data.0.iter().collect();
        entries.sort_by_key(|(command, _)| *command);
        entries
    }

    pub fn update(&mut self, command: &str, entry: CacheEntry) {
        debug!("updating cache entry for command '{command}': {entry:?}");
        self.data.0.insert(command.into(), entry);
        self.update = true;
    }

    pub fn delete(&mut self, command: &str) {
        debug!("deleting cache for command '{command}'");
        self.data.0.remove(command);
        self.update = true;
    }

    pub fn empty(&mut self) {
        debug!("emptying cache");
        self.data.0.clear();
        self.update = true;
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        if self.update {
            debug!("writing cache to disk: {}", self.path.display());
            let bytes = bitcode::encode(&self.data);
            if let Err(e) = fs::write(&self.path, bytes) {
                error!(
                    "failed to write cache to '{}': {e}",
                    display_path(&self.path)
                );
            }
        }
    }
}
