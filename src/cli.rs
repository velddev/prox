use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const HELP: &str = "prox — TCP forwarding to your dev machine

Usage:
  prox                       Pick a saved environment interactively
  prox -p 3000,12345,25565 [-d] [--host HOST]
  prox -p 3000:8080,25565
  prox status
  prox stop
  prox install
  prox -e NAME [-d]
  prox env [list]
  prox env add NAME
  prox env remove NAME
  prox env NAME host set HOST
  prox env NAME ports set LIST
  prox env NAME host clear
  prox env NAME ports clear
  prox env NAME get

Options:
  -p, --ports LIST   Comma-separated LOCAL[:REMOTE] ports
  -d, --daemon       Keep running after the terminal closes
  -e, --env NAME     Use a saved environment
      --host HOST    Destination IP or hostname (overrides saved host and PROX_HOST)
  -h, --help         Show help
  -V, --version      Show version

Listeners bind to 127.0.0.1. Foreground mode stops on Ctrl+C.
3000:8080 means localhost:3000 → destination:8080. TCP only.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Port {
    pub local: u16,
    pub remote: u16,
}

#[derive(Debug)]
pub enum Command {
    Run {
        ports: Option<Vec<Port>>,
        host: Option<String>,
        environment: Option<String>,
        daemon: bool,
        worker: bool,
    },
    Status,
    Stop,
    Install,
    Env(EnvCommand),
    Interactive,
    Help,
    Version,
}

#[derive(Debug)]
pub enum EnvCommand {
    List,
    Add(String),
    Remove(String),
    Get(String),
    Host(String, Option<String>),
    Ports(String, Option<Vec<Port>>),
}

pub fn environment_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 64
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        || ["add", "remove", "list", "help"].contains(&name)
    {
        return Err("environment names must start with a letter or number, use only letters, numbers, _ or -, and be at most 64 characters; add/remove/list/help are reserved".into());
    }
    Ok(())
}

pub fn validate_host(host: &str) -> Result<(), String> {
    if host.is_empty()
        || host
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '/' || c == '\\')
    {
        return Err("host must be an IP address or hostname, without a URL or whitespace".into());
    }
    Ok(())
}

fn parse_env(args: &[String]) -> Result<EnvCommand, String> {
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    let (name, command) = match args.as_slice() {
        [] | ["list"] => return Ok(EnvCommand::List),
        ["add", name] => (*name, EnvCommand::Add((*name).into())),
        ["remove", name] => (*name, EnvCommand::Remove((*name).into())),
        [name, "get"] => (*name, EnvCommand::Get((*name).into())),
        [name, "host", "set", value] => {
            validate_host(value)?;
            (
                *name,
                EnvCommand::Host((*name).into(), Some((*value).into())),
            )
        }
        [name, "ports", "set", value] => (
            *name,
            EnvCommand::Ports((*name).into(), Some(ports(value)?)),
        ),
        [name, "host", "clear"] => (*name, EnvCommand::Host((*name).into(), None)),
        [name, "ports", "clear"] => (*name, EnvCommand::Ports((*name).into(), None)),
        _ => return Err("invalid env command; see prox --help".into()),
    };
    environment_name(name)?;
    Ok(command)
}

pub fn ports(value: &str) -> Result<Vec<Port>, String> {
    let mut seen = HashSet::new();
    value
        .split(',')
        .map(|entry| {
            let (local, remote) = entry.split_once(':').unwrap_or((entry, entry));
            let number = |s: &str| {
                s.trim()
                    .parse::<u16>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| format!("invalid port in '{entry}': use 1–65535"))
            };
            let port = Port {
                local: number(local)?,
                remote: number(remote)?,
            };
            if !seen.insert(port.local) {
                return Err(format!("local port {} appears more than once", port.local));
            }
            Ok(port)
        })
        .collect()
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let args: Vec<_> = args.into_iter().collect();
    if args.first().is_some_and(|arg| arg == "env") {
        return parse_env(&args[1..]).map(Command::Env);
    }
    match args.as_slice() {
        [] => return Ok(Command::Interactive),
        [arg] if arg == "status" => return Ok(Command::Status),
        [arg] if arg == "stop" => return Ok(Command::Stop),
        [arg] if arg == "install" => return Ok(Command::Install),
        [arg] if arg == "-h" || arg == "--help" => return Ok(Command::Help),
        [arg] if arg == "-V" || arg == "--version" => return Ok(Command::Version),
        _ => (),
    }
    let mut args = args.into_iter();
    let (mut mappings, mut daemon, mut worker) = (None, false, false);
    let (mut host, mut environment) = (None, None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-p" | "--ports" => {
                if mappings.is_some() {
                    return Err("specify --ports only once".into());
                }
                mappings = Some(ports(&args.next().ok_or("--ports needs a value")?)?);
            }
            "--host" => host = Some(args.next().ok_or("--host needs a value")?),
            "-e" | "--env" => {
                if environment.is_some() {
                    return Err("specify --env only once".into());
                }
                let name = args.next().ok_or("--env needs a name")?;
                environment_name(&name)?;
                environment = Some(name);
            }
            "-d" | "--daemon" | "--detach" => daemon = true,
            "--worker" => worker = true,
            _ => return Err(format!("unknown argument '{arg}'; see prox --help")),
        }
    }
    if let Some(host) = &host {
        validate_host(host)?;
    }
    Ok(Command::Run {
        ports: mappings,
        host,
        environment,
        daemon,
        worker,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_mappings_have_local_first() {
        assert_eq!(
            ports("3000,12345:8080,25565").unwrap(),
            vec![
                Port {
                    local: 3000,
                    remote: 3000
                },
                Port {
                    local: 12345,
                    remote: 8080
                },
                Port {
                    local: 25565,
                    remote: 25565
                },
            ]
        );
    }

    #[test]
    fn rejects_ambiguous_or_invalid_mappings() {
        for value in [
            "",
            "0",
            "65536",
            "-1",
            "3000,",
            ":80",
            "80:",
            "1:2:3",
            "3000,3000:80",
            "http",
        ] {
            assert!(ports(value).is_err(), "accepted {value}");
        }
    }
}
