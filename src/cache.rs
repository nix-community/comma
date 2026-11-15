use std::{
    collections::HashMap,
    env,
    error::Error,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    process,
};

use bitcode::{Decode, Encode};
use log::{debug, error};

#[derive(Encode, Decode, Default)]
struct CacheData(HashMap<String, CacheEntry>);

#[derive(Encode, Decode, Clone, Debug)]
pub struct CacheEntry {
    pub derivation: String,
    /// Evaluated command paths, keyed by the nixpkgs source (flake ref, or
    /// `<nixpkgs>` with the `NIX_PATH` entry it was resolved through) they were
    /// built from
    pub paths: HashMap<String, String>,
}

impl CacheEntry {
    /// Remember `path` as the command path when built from `source`
    pub fn set_path(&mut self, source: &str, path: String) {
        // Nothing else removes the paths of sources that aren't used anymore,
        // so drop the ones that were garbage collected
        self.paths.retain(|_, path| Path::new(path).exists());
        self.paths.insert(source.to_owned(), path);
    }
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

/// The file next to `path` that is named like it, followed by `suffix`
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

pub struct Cache {
    path: PathBuf,
    data: CacheData,
    /// The changes made since the file was read, by command: the new entry,
    /// or `None` for a deleted one
    changes: HashMap<String, Option<CacheEntry>>,
    /// Whether the cache was emptied before `changes` were made
    emptied: bool,
}

impl Cache {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        Self::load(xdg::BaseDirectories::new()?.place_state_file("comma/choices")?)
    }

    fn load(path: PathBuf) -> Result<Self, Box<dyn Error>> {
        debug!("creating new cache instance for path: {}", path.display());

        let data = Self::read_file(&path)
            .map_err(|e| format!("can't read '{}': {e}", display_path(&path)))?;

        Ok(Self {
            data,
            path,
            changes: HashMap::new(),
            emptied: false,
        })
    }

    /// The contents of the cache file at `path`. A file that doesn't exist
    /// or is empty is an empty cache. So is one that can't be decoded, which
    /// is moved to `choices.old`, so that the next write doesn't overwrite it
    fn read_file(path: &Path) -> io::Result<CacheData> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };

        // Nothing in it that could be lost. Versions of comma that wrote the
        // file in place left it empty when the disk was full
        if bytes.is_empty() {
            return Ok(CacheData::default());
        }

        bitcode::decode(&bytes).or_else(|decode_err| {
            let old = sibling(path, ".old");
            match fs::rename(path, &old) {
                Ok(()) => {}
                // Another comma running at the same time has moved it
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(io::Error::other(format!(
                        "the existing file could not be decoded ({decode_err}) and \
                         could not be moved to '{}': {e}",
                        display_path(&old)
                    )))
                }
            }

            // Shown by default, since the user's package choices are lost
            eprintln!(
                "comma: the cache at '{}' could not be read: {decode_err}\n\
                 comma: it was moved to '{}'. The cache format may have changed \
                 between comma versions. Remembered package choices and cached \
                 paths are lost, so you will be asked to pick packages again.",
                display_path(path),
                display_path(&old)
            );
            Ok(CacheData::default())
        })
    }

    /// Write the changes to the cache file, in a way that works with another
    /// comma running at the same time (`, jq . | , fx`)
    fn write(&self) -> io::Result<()> {
        // Apply the changes to what is in the file now, not to what was read
        // at startup, so that entries the other comma has written since then
        // are kept
        let mut data = if self.emptied {
            CacheData::default()
        } else {
            Self::read_file(&self.path)?
        };
        for (command, entry) in &self.changes {
            match entry {
                Some(entry) => data.0.insert(command.clone(), entry.clone()),
                None => data.0.remove(command),
            };
        }

        // Write a temporary file and rename it over the cache file, so that
        // the other comma reads either the old or the new contents, never a
        // partially written file. It is named after the process, so that two
        // commas don't write to the same temporary file
        let tmp = sibling(&self.path, &format!(".{}.tmp", process::id()));

        File::create(&tmp)
            .and_then(|mut file| {
                file.write_all(&bitcode::encode(&data))?;
                // On the disk before the rename, so that a crash right after
                // it can't leave an empty cache file
                file.sync_all()
            })
            .and_then(|()| fs::rename(&tmp, &self.path))
            .inspect_err(|_| {
                let _ = fs::remove_file(&tmp);
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
        self.data.0.insert(command.into(), entry.clone());
        self.changes.insert(command.into(), Some(entry));
    }

    pub fn delete(&mut self, command: &str) {
        debug!("deleting cache for command '{command}'");
        self.data.0.remove(command);
        self.changes.insert(command.into(), None);
    }

    pub fn empty(&mut self) {
        debug!("emptying cache");
        self.data.0.clear();
        self.changes.clear();
        self.emptied = true;
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        if self.emptied || !self.changes.is_empty() {
            debug!("writing cache to disk: {}", self.path.display());
            if let Err(e) = self.write() {
                error!(
                    "failed to write cache to '{}': {e}",
                    display_path(&self.path)
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new, empty directory for the test called `name`
    fn test_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("comma-test-{}-{name}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entry(derivation: &str) -> CacheEntry {
        CacheEntry {
            derivation: derivation.to_owned(),
            paths: HashMap::new(),
        }
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn set_path_drops_garbage_collected_paths() {
        let dir = test_dir("set_path_drops_garbage_collected_paths");
        let existing = dir.join("existing").display().to_string();
        let gone = dir.join("gone").display().to_string();
        let new = dir.join("new").display().to_string();
        fs::write(&existing, b"").unwrap();

        let mut entry = entry("hello.out");
        entry.paths.insert("nixpkgs".to_owned(), existing.clone());
        entry.paths.insert("<nixpkgs> (/old)".to_owned(), gone);
        // The new path is stored without being checked
        entry.set_path("<nixpkgs> (/new)", new.clone());

        assert_eq!(
            entry.paths,
            HashMap::from([
                ("nixpkgs".to_owned(), existing),
                ("<nixpkgs> (/new)".to_owned(), new),
            ])
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_changes_are_merged() {
        let dir = test_dir("concurrent_changes_are_merged");
        let path = dir.join("choices");

        let mut cache = Cache::load(path.clone()).unwrap();
        cache.update("vi", entry("vim.out"));
        cache.update("hello", entry("hello.out"));
        drop(cache);

        // Two commas that both read the file before either of them writes it
        let mut first = Cache::load(path.clone()).unwrap();
        let mut second = Cache::load(path.clone()).unwrap();
        first.update("jq", entry("jq.bin"));
        first.delete("hello");
        second.update("fx", entry("fx.out"));
        drop(first);
        drop(second);

        let cache = Cache::load(path.clone()).unwrap();
        let commands: Vec<_> = cache.entries().into_iter().map(|(c, _)| c).collect();
        assert_eq!(commands, ["fx", "jq", "vi"]);

        // Emptying the cache also removes what was written in the meantime
        let mut first = Cache::load(path.clone()).unwrap();
        let mut second = Cache::load(path.clone()).unwrap();
        first.update("hello", entry("hello.out"));
        second.empty();
        second.update("rg", entry("ripgrep.out"));
        drop(first);
        drop(second);

        let cache = Cache::load(path).unwrap();
        let commands: Vec<_> = cache.entries().into_iter().map(|(c, _)| c).collect();
        assert_eq!(commands, ["rg"]);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn empty_file_is_an_empty_cache() {
        let dir = test_dir("empty_file_is_an_empty_cache");
        let path = dir.join("choices");
        fs::write(&path, b"").unwrap();

        // Nothing to keep a backup of
        assert!(Cache::load(path).unwrap().entries().is_empty());
        assert_eq!(file_names(&dir), ["choices"]);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn undecodable_file_is_kept_as_backup() {
        let dir = test_dir("undecodable_file_is_kept_as_backup");
        let path = dir.join("choices");
        fs::write(&path, b"garbage").unwrap();

        // Starts empty instead of failing, with the file moved out of the way
        let mut cache = Cache::load(path.clone()).unwrap();
        assert!(cache.entries().is_empty());
        assert_eq!(file_names(&dir), ["choices.old"]);
        assert_eq!(fs::read(dir.join("choices.old")).unwrap(), b"garbage");
        cache.update("hello", entry("hello.out"));
        drop(cache);

        // The update wrote a new file and left no temporary file behind
        assert!(Cache::load(path).unwrap().query("hello").is_some());
        assert_eq!(file_names(&dir), ["choices", "choices.old"]);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn undecodable_file_that_cannot_be_moved_is_left_alone() {
        let dir = test_dir("undecodable_file_that_cannot_be_moved_is_left_alone");
        let path = dir.join("choices");
        fs::write(&path, b"garbage").unwrap();
        // A file can't be renamed over a directory
        fs::create_dir(dir.join("choices.old")).unwrap();

        // The cache is disabled rather than the file overwritten
        let err = Cache::load(path.clone()).err().unwrap().to_string();
        assert!(err.contains("could not be moved to"), "{err}");
        assert_eq!(fs::read(&path).unwrap(), b"garbage");

        fs::remove_dir_all(dir).unwrap();
    }
}
