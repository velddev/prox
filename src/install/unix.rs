use super::{Result, absolute_env};
use std::{
    env,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub struct Plan {
    directory: PathBuf,
    profiles: Vec<PathBuf>,
    block: String,
}

impl Plan {
    pub fn new() -> Result<Self> {
        let home = absolute_env("HOME")?;
        let directory = if cfg!(target_os = "macos") {
            home.join("Applications/prox/bin")
        } else {
            home.join(".local/bin")
        };
        let shell = env::var("SHELL").unwrap_or_else(|_| {
            if cfg!(target_os = "macos") {
                "/bin/zsh"
            } else {
                "/bin/bash"
            }
            .into()
        });
        let shell = Path::new(&shell)
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("cannot identify your shell")?;
        let fish = shell == "fish";
        let profiles =
            match shell {
                "zsh" => {
                    let directory = if env::var_os("ZDOTDIR").is_some() {
                        absolute_env("ZDOTDIR")?
                    } else {
                        home.clone()
                    };
                    vec![directory.join(".zprofile"), directory.join(".zshrc")]
                }
                "bash" => {
                    let login = [".bash_profile", ".bash_login", ".profile"]
                        .into_iter()
                        .map(|name| home.join(name))
                        .find(|path| path.exists())
                        .unwrap_or_else(|| home.join(".profile"));
                    vec![login, home.join(".bashrc")]
                }
                "sh" | "dash" | "ksh" => vec![home.join(".profile")],
                "fish" => {
                    let config =
                        match env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
                            Some(_) => absolute_env("XDG_CONFIG_HOME")?,
                            None => home.join(".config"),
                        };
                    vec![config.join("fish/conf.d/prox.fish")]
                }
                _ => return Err(format!(
                    "automatic PATH setup supports bash, zsh, fish, sh, dash and ksh; found {shell}"
                )
                .into()),
            };
        let path = directory
            .to_str()
            .ok_or("install path must be valid UTF-8")?;
        if path.contains(['\n', '\r', ':']) {
            return Err("install path cannot contain a newline or colon".into());
        }
        let quoted = if fish {
            format!("'{}'", path.replace('\\', "\\\\").replace('\'', "\\'"))
        } else {
            format!("'{}'", path.replace('\'', "'\\''"))
        };
        let command = if fish {
            format!("if not contains -- {quoted} $PATH\n    set -gx PATH {quoted} $PATH\nend")
        } else {
            format!(
                "case \":$PATH:\" in\n    *:{quoted}:*) ;;\n    *) export PATH={quoted}:\"$PATH\" ;;\nesac"
            )
        };
        let block = format!("# >>> prox PATH >>>\n{command}\n# <<< prox PATH <<<\n");
        // Check existing files before copying the executable or changing any profile.
        for profile in &profiles {
            if profile.exists() {
                check_block(&fs::read_to_string(profile)?, &block)?;
            }
        }
        Ok(Self {
            directory,
            profiles,
            block,
        })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn configure_path(&self) -> Result<()> {
        for profile in &self.profiles {
            if let Some(parent) = profile.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut file = OpenOptions::new()
                .read(true)
                .append(true)
                .create(true)
                .open(profile)?;
            file.lock()?;
            let mut content = String::new();
            file.read_to_string(&mut content)?;
            check_block(&content, &self.block)?;
            if content.contains(&self.block) {
                continue;
            }
            if !content.is_empty() && !content.ends_with('\n') {
                file.write_all(b"\n")?;
            }
            file.write_all(self.block.as_bytes())?;
        }
        Ok(())
    }
}

fn check_block(content: &str, block: &str) -> Result<()> {
    if content.contains("# >>> prox PATH >>>") && !content.contains(block) {
        return Err("an existing prox PATH block was edited or points elsewhere; remove that block and rerun prox install".into());
    }
    Ok(())
}
