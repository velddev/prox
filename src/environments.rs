use crate::{
    Result,
    cli::{self, EnvCommand, Port},
    state::private_options,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env,
    fs::{self, File},
    io::{self, BufRead, Write},
    path::PathBuf,
};

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Environment {
    #[serde(skip_serializing_if = "Option::is_none")]
    host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ports: Option<Vec<Port>>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    environments: BTreeMap<String, Environment>,
}

struct Store {
    directory: PathBuf,
    _lock: File,
    config: Config,
}

impl Store {
    fn open() -> Result<Self> {
        let directory = if let Some(path) = env::var_os("PROX_CONFIG_DIR").filter(|p| !p.is_empty())
        {
            PathBuf::from(path)
        } else if cfg!(windows) {
            PathBuf::from(env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is not set")?)
                .join("prox")
        } else if let Some(path) = env::var_os("XDG_CONFIG_HOME").filter(|p| !p.is_empty()) {
            PathBuf::from(path).join("prox")
        } else {
            PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?).join(".config/prox")
        };
        if !directory.is_absolute() {
            return Err("configuration directory must be an absolute path".into());
        }
        fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
        let lock = private_options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("environments.lock"))?;
        lock.lock()?;
        let file = directory.join("environments.json");
        let config: Config = match fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                format!(
                    "cannot read {}: {e}; existing config was left unchanged",
                    file.display()
                )
            })?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Config::default(),
            Err(error) => return Err(error.into()),
        };
        for (name, saved) in &config.environments {
            cli::environment_name(name)?;
            if let Some(host) = &saved.host {
                cli::validate_host(host)?;
            }
            if let Some(ports) = &saved.ports {
                cli::ports(&format_ports(ports))?;
            }
        }
        Ok(Self {
            directory,
            _lock: lock,
            config,
        })
    }

    fn get(&self, name: &str) -> Result<&Environment> {
        self.config
            .environments
            .get(name)
            .ok_or_else(|| format!("environment '{name}' does not exist").into())
    }

    fn save(&self) -> Result<()> {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).map_err(|e| format!("cannot save environment: {e}"))?;
        let temporary = self.directory.join(format!(
            ".environments-{:016x}.tmp",
            u64::from_ne_bytes(random)
        ));
        let result = (|| -> Result<()> {
            let mut file = private_options()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            serde_json::to_writer_pretty(&mut file, &self.config)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, self.directory.join("environments.json"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

pub fn execute(command: EnvCommand) -> Result<()> {
    let mut store = Store::open()?;
    match command {
        EnvCommand::List => {
            if store.config.environments.is_empty() {
                println!("No saved environments. Add one with: prox env add NAME");
            }
            for name in store.config.environments.keys() {
                println!("{name}");
            }
            return Ok(());
        }
        EnvCommand::Get(name) => {
            let saved = store.get(&name)?;
            println!(
                "{name}\n  host: {}\n  ports: {}",
                saved.host.as_deref().unwrap_or("(not set)"),
                saved
                    .ports
                    .as_deref()
                    .map(format_ports)
                    .unwrap_or_else(|| "(not set)".into())
            );
            return Ok(());
        }
        EnvCommand::Add(name) => {
            if store.config.environments.contains_key(&name) {
                return Err(format!("environment '{name}' already exists").into());
            }
            store
                .config
                .environments
                .insert(name.clone(), Environment::default());
            store.save()?;
            println!("Added environment '{name}'.");
        }
        EnvCommand::Remove(name) => {
            store.get(&name)?;
            store.config.environments.remove(&name);
            store.save()?;
            println!("Removed environment '{name}'.");
        }
        EnvCommand::Host(name, host) => {
            store.get(&name)?;
            store.config.environments.get_mut(&name).unwrap().host = host;
            store.save()?;
            println!("Updated host for '{name}'.");
        }
        EnvCommand::Ports(name, ports) => {
            store.get(&name)?;
            store.config.environments.get_mut(&name).unwrap().ports = ports;
            store.save()?;
            println!("Updated ports for '{name}'.");
        }
    }
    Ok(())
}

pub fn resolve(
    ports: Option<Vec<Port>>,
    host: Option<String>,
    name: Option<&str>,
) -> Result<(Vec<Port>, String)> {
    let saved = match name {
        Some(name) => Store::open()?.get(name)?.clone(),
        None => Environment::default(),
    };
    let host = host
        .or(saved.host)
        .or_else(|| env::var("PROX_HOST").ok())
        .ok_or("missing destination; use --host HOST, set an environment host, or set PROX_HOST")?;
    cli::validate_host(&host)?;
    let ports = ports
        .or(saved.ports)
        .ok_or("missing ports; use -p PORTS or set an environment's ports")?;
    Ok((ports, host))
}

fn format_ports(ports: &[Port]) -> String {
    ports
        .iter()
        .map(|port| {
            if port.local == port.remote {
                port.local.to_string()
            } else {
                format!("{}:{}", port.local, port.remote)
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub fn select() -> Result<Option<String>> {
    let names: Vec<_> = Store::open()?.config.environments.into_keys().collect();
    let (mut input, mut output) = (io::stdin().lock(), io::stdout().lock());
    if names.is_empty() {
        writeln!(output, "No saved environments. Create one to get started.")?;
        return create_interactively(&names, &mut input, &mut output);
    }
    match select_index(&names, &mut input, &mut output)? {
        Some(index) if index == names.len() => {
            create_interactively(&names, &mut input, &mut output)
        }
        Some(index) => Ok(Some(names[index].clone())),
        None => Ok(None),
    }
}

fn create_interactively(
    names: &[String],
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<Option<String>> {
    writeln!(output, "Create an environment (q to cancel).")?;
    let Some(name) = prompt("Name", input, output, |name| {
        cli::environment_name(name)?;
        if names.iter().any(|existing| existing == name) {
            return Err("that environment already exists".into());
        }
        Ok(())
    })?
    else {
        return Ok(None);
    };
    let Some(host) = prompt("Host", input, output, cli::validate_host)? else {
        return Ok(None);
    };
    let Some(ports) = prompt(
        "Ports (LOCAL[:REMOTE], comma-separated)",
        input,
        output,
        |ports| cli::ports(ports).map(|_| ()),
    )?
    else {
        return Ok(None);
    };
    let ports = cli::ports(&ports)?;
    // Save all fields together after the last prompt. Never hold a file lock while waiting for input.
    let mut store = Store::open()?;
    if store.config.environments.contains_key(&name) {
        return Err(format!("environment '{name}' was created by another process; existing config was left unchanged").into());
    }
    store.config.environments.insert(
        name.clone(),
        Environment {
            host: Some(host),
            ports: Some(ports),
        },
    );
    store.save()?;
    writeln!(output, "Saved environment '{name}'.")?;
    Ok(Some(name))
}

fn prompt(
    label: &str,
    input: &mut impl BufRead,
    output: &mut impl Write,
    validate: impl Fn(&str) -> std::result::Result<(), String>,
) -> io::Result<Option<String>> {
    loop {
        write!(output, "{label}: ")?;
        output.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let value = line.trim();
        if value == "q" {
            return Ok(None);
        }
        match validate(value) {
            Ok(()) => return Ok(Some(value.into())),
            Err(error) => writeln!(output, "{error}")?,
        }
    }
}

fn select_index(
    names: &[String],
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<Option<usize>> {
    writeln!(output, "Pick an environment:")?;
    for (index, name) in names.iter().enumerate() {
        writeln!(output, "  {}) {name}", index + 1)?;
    }
    writeln!(output, "  {}) create a new environment", names.len() + 1)?;
    loop {
        write!(output, "Environment (number/name, q to quit): ")?;
        output.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let value = line.trim();
        if value == "q" {
            return Ok(None);
        }
        if let Some(index) = names.iter().position(|name| name == value) {
            return Ok(Some(index));
        }
        if let Some(index) = value
            .parse::<usize>()
            .ok()
            .and_then(|number| number.checked_sub(1))
            .filter(|index| *index <= names.len())
        {
            return Ok(Some(index));
        }
        writeln!(output, "Choose a number or name from the list.")?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_accepts_names_and_numbers_and_retries_invalid_input() {
        let names = vec!["dev".into(), "staging".into()];
        for (input, expected) in [
            ("2\n", Some(1)),
            ("3\n", Some(2)),
            ("0\nunknown\ndev\n", Some(0)),
            ("q\n", None),
            ("", None),
        ] {
            assert_eq!(
                select_index(&names, &mut input.as_bytes(), &mut Vec::new()).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn creation_prompts_validate_and_allow_cancellation() {
        let mut output = Vec::new();
        let mut input = b"bad host\nexample.com\n".as_slice();
        assert_eq!(
            prompt("Host", &mut input, &mut output, cli::validate_host).unwrap(),
            Some("example.com".into())
        );
        assert!(
            String::from_utf8(output)
                .unwrap()
                .contains("without a URL or whitespace")
        );
        assert_eq!(
            prompt(
                "Host",
                &mut b"q\n".as_slice(),
                &mut Vec::new(),
                cli::validate_host
            )
            .unwrap(),
            None
        );
    }
}
