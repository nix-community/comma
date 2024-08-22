use std::{collections::HashMap, error::Error, fs, path::PathBuf};

use bitcode::{Decode, Encode};

#[derive(Encode, Decode)]
struct CacheData(HashMap<String, String>);

pub struct Cache {
    path: PathBuf,
    data: CacheData,
    update: bool,
}

impl Cache {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let path = xdg::BaseDirectories::new()?.place_state_file("comma-choices")?;

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

    pub fn query(&self, command: &str) -> Option<String> {
        self.data.0.get(command).cloned()
    }

    pub fn update(&mut self, command: &str, derivation: &str) {
        self.data.0.insert(command.into(), derivation.into());
        self.update = true;
    }

    pub fn delete(&mut self, command: &str) {
        self.data.0.remove(command);
        self.update = true;
    }

    pub fn empty(&mut self) {
        self.data.0.clear();
        self.update = true;
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        if self.update {
            let bytes = bitcode::encode(&self.data.0);
            if let Err(e) = fs::write(&self.path, bytes) {
                eprintln!("failed to write cache: {e}");
            }
        }
    }
}
