use std::{collections::HashMap, error::Error, fs, path::PathBuf};

use bitcode::{Decode, Encode};
use log::debug;

#[derive(Encode, Decode)]
struct CacheData(HashMap<String, CacheEntry>);

#[derive(Encode, Decode, Clone, Debug)]
pub struct CacheEntry {
    pub derivation: String,
    pub path: Option<String>,
}

pub struct Cache {
    path: PathBuf,
    data: CacheData,
    update: bool,
}

impl Cache {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        debug!("creating new cache instance");

        let path = xdg::BaseDirectories::new()?.place_state_file("comma/choices")?;

        Ok(Self {
            data: if path.exists() {
                let bytes = fs::read(&path)?;
                bitcode::decode(&bytes)?
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
            let bytes = bitcode::encode(&self.data.0);
            if let Err(e) = fs::write(&self.path, bytes) {
                eprintln!("failed to write cache: {e}");
            }
        }
    }
}
