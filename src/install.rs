use crate::Result;
use std::{
    env,
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

#[cfg(unix)]
#[path = "install/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "install/windows.rs"]
mod platform;

pub fn run() -> Result<()> {
    let plan = platform::Plan::new()?;
    let source = env::current_exe()?;
    let destination = plan
        .directory()
        .join(if cfg!(windows) { "prox.exe" } else { "prox" });
    copy_executable(&source, &destination)?;
    plan.configure_path()?;
    println!("Installed prox to {}", destination.display());
    println!("PATH is set up. Reopen your terminal, then run prox --help.");
    Ok(())
}

pub(super) fn absolute_env(name: &str) -> Result<PathBuf> {
    let path = PathBuf::from(env::var_os(name).ok_or_else(|| format!("{name} is not set"))?);
    if !path.is_absolute() {
        return Err(format!("{name} must be an absolute path").into());
    }
    Ok(path)
}

fn copy_executable(source: &Path, destination: &Path) -> Result<()> {
    if destination.exists() {
        if fs::canonicalize(source)? == fs::canonicalize(destination)? {
            return Ok(());
        }
        // Don't replace an identical executable, especially one Windows has open.
        if fs::read(source)? == fs::read(destination)? {
            return Ok(());
        }
    }
    let directory = destination.parent().ok_or("missing install directory")?;
    fs::create_dir_all(directory)?;
    let mut random = [0u8; 8];
    getrandom::fill(&mut random)
        .map_err(|e| format!("cannot create installer staging file: {e}"))?;
    let temporary = directory.join(format!(".prox-{:016x}.tmp", u64::from_ne_bytes(random)));
    let result = (|| -> Result<()> {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        io::copy(&mut File::open(source)?, &mut output)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            output.set_permissions(fs::Permissions::from_mode(0o755))?;
        }
        output.sync_all()?;
        drop(output);
        fs::rename(&temporary, destination).map_err(|e| {
            format!(
                "cannot install {}: {e}. If prox is running, stop it first",
                destination.display()
            )
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
