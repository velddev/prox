use crate::cli::Port;
use serde::{Deserialize, Serialize};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::PathBuf,
};

#[derive(Clone)]
pub struct Paths {
    pub dir: PathBuf,
}

impl Paths {
    pub fn new() -> io::Result<Self> {
        let dir = if let Some(path) = env::var_os("PROX_STATE_DIR") {
            PathBuf::from(path)
        } else if cfg!(windows) {
            PathBuf::from(
                env::var_os("LOCALAPPDATA")
                    .ok_or_else(|| io::Error::other("LOCALAPPDATA is not set"))?,
            )
            .join("prox")
        } else if let Some(path) = env::var_os("XDG_STATE_HOME") {
            PathBuf::from(path).join("prox")
        } else {
            PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is not set"))?)
                .join(".local/state/prox")
        };
        fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self { dir })
    }

    pub fn lock(&self) -> io::Result<Option<File>> {
        let file = private_options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.dir.join("lock"))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => Err(error),
        }
    }

    pub fn read(&self) -> io::Result<State> {
        Ok(serde_json::from_slice(&fs::read(
            self.dir.join("state.json"),
        )?)?)
    }

    pub fn publish(&self, state: &State) -> io::Result<()> {
        let mut file = private_options()
            .write(true)
            .create(true)
            .truncate(true)
            .open(self.dir.join("state.json"))?;
        file.write_all(&serde_json::to_vec(state)?)
    }
}

pub fn private_options() -> OpenOptions {
    let options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = options;
        options.mode(0o600);
        options
    }
    #[cfg(not(unix))]
    options
}

#[derive(Serialize, Deserialize)]
pub struct State {
    pub pid: u32,
    pub control_port: u16,
    pub token: String,
    pub host: String,
    pub ports: Vec<Port>,
}

pub struct Guard {
    pub paths: Paths,
    pub _lock: File,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.paths.dir.join("state.json"));
    }
}
