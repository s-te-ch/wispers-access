use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::io::Write; // For .write_all()
use std::path::PathBuf;
use tempfile::NamedTempFile;
use wispers_connect as wc;

//-- ShareConfig format --------------------------------------------------------

const SHARE_CONFIG_FILENAME: &str = "share_config.json";
const CURRENT_VERSION: u32 = 1;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ShareConfig {
    /// Schema version. Doesn't need to be set until we actually have a breaking
    /// change and need version 2.
    #[serde(default = "default_version")]
    version: u32,

    /// API key for the Wispers domain, used to create new node registrations.
    pub api_key: String,

    /// ID of the Wispers connectivity group that corresponds to this share.
    pub connectivity_group_id: String,
}

impl ShareConfig {
    pub fn new(api_key: &str, connectivity_group_id: &str) -> Self {
        Self {
            version: default_version(),
            api_key: api_key.to_owned(),
            connectivity_group_id: connectivity_group_id.to_owned(),
        }
    }
}

fn default_version() -> u32 {
    1
}

// Custom debug implementation so we don't leak API key.
impl std::fmt::Debug for ShareConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShareConfig")
            .field("version", &self.version)
            .field("api_key", &"[redacted]")
            .finish()
    }
}

//-- Error type ----------------------------------------------------------------

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    #[error("parse error at {}: {}", .0.path(), .0.inner())]
    Parse(#[from] serde_path_to_error::Error<serde_json::Error>),

    #[error("failed to serialize config: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("unsupported config version {0} (supporting ≤ {CURRENT_VERSION})")]
    UnsupportedVersion(u32),

    #[error("could not determine the OS config directory")]
    NoConfigDir,
}

//-- Global functions ----------------------------------------------------------

pub fn list_shares() -> Result<Vec<String>, Error> {
    let path = base_dir()?;
    let shares: Vec<String> = if !path.exists() {
        Vec::new()
    } else {
        fs::read_dir(path)?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    };
    Ok(shares)
}

//-- Share state store ---------------------------------------------------------

#[derive(Clone)]
pub struct ShareStateStore {
    dir: PathBuf,
}

impl ShareStateStore {
    pub fn new(share: &str) -> Result<Self, Error> {
        Ok(Self {
            dir: config_dir(share)?,
        })
    }

    pub fn load_share_config(&self) -> Result<Option<ShareConfig>, Error> {
        let path = self.dir.join(SHARE_CONFIG_FILENAME);
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let de = &mut serde_json::Deserializer::from_slice(&bytes);
        let cfg: ShareConfig = serde_path_to_error::deserialize(de)?;
        if cfg.version > CURRENT_VERSION {
            return Err(Error::UnsupportedVersion(cfg.version));
        }
        Ok(Some(cfg))
    }

    pub fn save_share_config(&self, cfg: &ShareConfig) -> Result<(), Error> {
        let bytes = serde_json::to_vec_pretty(cfg)?;
        ensure_dir_exists(&self.dir)?;
        write_atomically(&self.dir, SHARE_CONFIG_FILENAME, &bytes)?;
        Ok(())
    }

    pub fn delete(&self) -> Result<(), Error> {
        match fs::remove_dir_all(&self.dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::Io(e)),
        }
    }
}

const ROOT_KEY_FILENAME: &str = "root_key.bin";
const REGISTRATION_FILENAME: &str = "registration.pb";

impl wc::NodeStateStore for ShareStateStore {
    fn load(&self) -> Result<Option<wc::PersistedNodeState>, wc::StorageError> {
        let root_key_path = self.dir.join(ROOT_KEY_FILENAME);
        let registration_path = self.dir.join(REGISTRATION_FILENAME);

        // Load root key
        if !root_key_path.exists() {
            return Ok(None);
        }
        let root_key_bytes = fs::read(&root_key_path)?;
        if root_key_bytes.len() != wc::ROOT_KEY_LEN {
            return Err(wc::StorageError::InvalidRootKey);
        }
        let mut key_array = [0u8; wc::ROOT_KEY_LEN];
        key_array.copy_from_slice(&root_key_bytes);

        // Load registration if present
        let registration = if registration_path.exists() {
            let bytes = fs::read(&registration_path)?;
            let reg = wc::deserialize_registration(&bytes)?;
            Some(reg)
        } else {
            None
        };

        Ok(Some(wc::PersistedNodeState::from_stored(
            key_array,
            registration,
        )))
    }

    fn save(&self, state: &wc::PersistedNodeState) -> Result<(), wc::StorageError> {
        ensure_dir_exists(&self.dir)?;
        write_atomically(&self.dir, ROOT_KEY_FILENAME, state.root_key_bytes())?;
        if let Some(reg) = state.registration() {
            let bytes = wc::serialize_registration(reg);
            write_atomically(&self.dir, REGISTRATION_FILENAME, &bytes)?;
        } else {
            let path = self.dir.join(REGISTRATION_FILENAME);
            if path.exists() {
                fs::remove_file(&path)?;
            }
        }
        Ok(())
    }

    fn delete(&self) -> Result<(), wc::StorageError> {
        let root_key_path = self.dir.join(ROOT_KEY_FILENAME);
        let registration_path = self.dir.join(REGISTRATION_FILENAME);
        if root_key_path.exists() {
            fs::remove_file(&root_key_path)?;
        }
        if registration_path.exists() {
            fs::remove_file(&registration_path)?;
        }
        Ok(())
    }
}

fn config_dir(share: &str) -> Result<PathBuf, Error> {
    let dir = base_dir()?.join(share);
    Ok(dir)
}

fn base_dir() -> Result<PathBuf, Error> {
    let dir = dirs::config_dir()
        .ok_or(Error::NoConfigDir)?
        .join("waserver");
    Ok(dir)
}

fn ensure_dir_exists(dir: &PathBuf) -> Result<(), std::io::Error> {
    if !dir.exists() {
        fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

fn write_atomically(dir: &PathBuf, name: &str, data: &[u8]) -> Result<(), std::io::Error> {
    let path = dir.join(name);
    let mut tmp = NamedTempFile::new_in(dir)?;
    tmp.write_all(data)?;
    tmp.as_file().sync_all()?;
    tmp.persist(&path).map_err(|e| e.error)?;
    #[cfg(unix)]
    {
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}
