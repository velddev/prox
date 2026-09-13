mod cli;
mod environments;
mod install;
mod state;

use cli::{Command, Port};
use state::{Guard, Paths, State};
use std::{
    env,
    error::Error,
    io::{self, BufRead, IsTerminal, Write},
    process::{self, Stdio},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    task::JoinSet,
    time::timeout,
};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

fn main() {
    let args: Vec<_> = env::args().skip(1).collect();
    let worker = args.iter().any(|arg| arg == "--worker");
    if let Err(error) = execute(args) {
        if worker {
            // Startup errors reach the launcher; after readiness that pipe is closed.
            let _ = writeln!(io::stdout(), "error: {error}");
            eprintln!("prox: {error}");
        } else {
            eprintln!("prox: {error}");
        }
        process::exit(1);
    }
}

fn execute(args: Vec<String>) -> Result<()> {
    let command = match cli::parse(args)? {
        Command::Interactive => {
            if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
                println!("{}", cli::HELP);
                return Ok(());
            }
            let Some(environment) = environments::select()? else {
                return Ok(());
            };
            Command::Run {
                ports: None,
                host: None,
                environment: Some(environment),
                daemon: false,
                worker: false,
            }
        }
        command => command,
    };
    match command {
        Command::Help => {
            println!("{}", cli::HELP);
            return Ok(());
        }
        Command::Version => {
            println!("prox {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Command::Install => return install::run(),
        Command::Env(command) => return environments::execute(command),
        _ => (),
    }
    let paths = Paths::new()?;
    // Resolve once before detaching, so later config edits cannot change this launch.
    let command = match command {
        Command::Run {
            ports,
            host,
            environment,
            daemon,
            worker,
        } => {
            let (ports, host) = environments::resolve(ports, host, environment.as_deref())?;
            if daemon && !worker {
                return detach(&paths, &ports, &host);
            }
            return tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(serve(paths, ports, host, worker));
        }
        command => command,
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            match command {
                Command::Status => inspect(&paths, false).await,
                Command::Stop => inspect(&paths, true).await,
                _ => unreachable!(),
            }
        })
}

fn describe(state: &State) {
    println!("prox running (PID {})", state.pid);
    for port in &state.ports {
        println!(
            "  127.0.0.1:{} → {}:{}",
            port.local, state.host, port.remote
        );
    }
}

fn detach(paths: &Paths, ports: &[Port], host: &str) -> Result<()> {
    let mappings = ports
        .iter()
        .map(|p| format!("{}:{}", p.local, p.remote))
        .collect::<Vec<_>>()
        .join(",");
    let log = state::private_options()
        .create(true)
        .append(true)
        .open(paths.dir.join("prox.log"))?;
    let mut command = process::Command::new(env::current_exe()?);
    command
        .args(["-p", &mappings, "--host", host, "--worker"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No inherited console; closing the terminal does not stop the daemon.
        command.creation_flags(0x00000008 | 0x00000200);
        detach_standard_handles()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe; no allocations or locks in pre_exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command.spawn()?;
    let output = child.stdout.take().ok_or("missing startup pipe")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = io::BufReader::new(output)
            .read_line(&mut line)
            .map(|_| line);
        let _ = tx.send(result);
    });
    let startup = rx.recv_timeout(Duration::from_secs(10));
    match startup {
        Ok(Ok(line)) if line.trim() == "ready" => {
            describe(&paths.read()?);
            println!("Daemon started. Stop with: prox stop");
            Ok(())
        }
        result => {
            let _ = child.kill();
            let _ = child.wait();
            let message = match result {
                Ok(Ok(line)) if !line.trim().is_empty() => line.trim().to_owned(),
                _ => format!(
                    "daemon did not start; see {}",
                    paths.dir.join("prox.log").display()
                ),
            };
            Err(message.into())
        }
    }
}

#[cfg(windows)]
fn detach_standard_handles() -> io::Result<()> {
    use windows_sys::Win32::{
        Foundation::{HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation},
        System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE},
    };
    // Command creates inheritable copies for the child's chosen stdio, but Windows
    // also inherits any other handles marked inheritable. Clear the launcher's
    // original stdio flags so a daemon cannot keep its caller's capture pipes open.
    for kind in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: GetStdHandle returns borrowed process-owned handles. We only
        // change inheritance flags, before starting any threads; we don't close them.
        unsafe {
            let handle = GetStdHandle(kind);
            if !handle.is_null()
                && handle != INVALID_HANDLE_VALUE
                && SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) == 0
            {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

async fn inspect(paths: &Paths, stop: bool) -> Result<()> {
    if paths.lock()?.is_some() {
        println!("prox is not running");
        return Ok(());
    }
    let state = paths
        .read()
        .map_err(|e| format!("prox is starting or its state is unavailable: {e}"))?;
    let verb = if stop { "stop" } else { "status" };
    let response = timeout(Duration::from_secs(3), async {
        let mut connection =
            TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, state.control_port)).await?;
        connection
            .write_all(format!("{} {verb}\n", state.token).as_bytes())
            .await?;
        let mut line = String::new();
        BufReader::new(connection.take(128))
            .read_line(&mut line)
            .await?;
        Ok::<_, io::Error>(line)
    })
    .await??;
    if response != "ok\n" {
        return Err("could not authenticate the running prox process".into());
    }
    if stop {
        for _ in 0..100 {
            if paths.lock()?.is_some() {
                println!("prox stopped");
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        return Err("prox did not finish stopping within 5 seconds".into());
    }
    describe(&state);
    Ok(())
}

async fn serve(paths: Paths, ports: Vec<Port>, host: String, worker: bool) -> Result<()> {
    let lock = paths
        .lock()?
        .ok_or("prox is already running; use prox status or prox stop")?;
    let guard = Guard { paths, _lock: lock };
    let mut listeners = Vec::new();
    // Bind everything before publishing readiness. A collision leaves no partial service.
    for port in &ports {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port.local))
            .await
            .map_err(|e| format!("cannot listen on 127.0.0.1:{}: {e}", port.local))?;
        listeners.push((listener, port.remote));
    }
    let control = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|e| format!("cannot generate control token: {e}"))?;
    let token: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let state = State {
        pid: process::id(),
        control_port: control.local_addr()?.port(),
        token: token.clone(),
        host: host.clone(),
        ports,
    };
    let shutdown = shutdown_signal()?;
    let (stop_tx, mut stop_rx) = tokio::sync::mpsc::channel(1);
    let mut tasks = JoinSet::new();
    for (listener, remote) in listeners {
        tasks.spawn(forward(listener, host.clone(), remote));
    }
    tasks.spawn(async move {
        let mut clients = JoinSet::new();
        loop {
            tokio::select! {
                accepted = control.accept() => {
                    let (stream, _) = accepted?;
                    let token = token.clone();
                    let stop_tx = stop_tx.clone();
                    clients.spawn(async move {
                        let _ = timeout(Duration::from_secs(2), async {
                            let mut reader = BufReader::new(stream.take(128));
                            let mut line = String::new();
                            reader.read_line(&mut line).await?;
                            let stop = line.trim_end() == format!("{token} stop");
                            if stop || line.trim_end() == format!("{token} status") {
                                reader.get_mut().get_mut().write_all(b"ok\n").await?;
                                if stop { let _ = stop_tx.send(()).await; }
                            }
                            Ok::<_, io::Error>(())
                        }).await;
                    });
                }
                _ = clients.join_next(), if !clients.is_empty() => (),
            }
        }
        #[allow(unreachable_code)]
        Ok::<_, io::Error>(())
    });
    guard.paths.publish(&state)?;
    if worker {
        println!("ready");
        io::stdout().flush()?;
    } else {
        describe(&state);
        println!("Ctrl+C to stop.");
    }
    let result = tokio::select! {
        result = shutdown => result.map_err(Into::into),
        _ = stop_rx.recv() => Ok(()),
        task = tasks.join_next() => match task {
            Some(Ok(Err(error))) => Err(error.into()),
            Some(Err(error)) => Err(error.into()),
            _ => Err("listener stopped unexpectedly".into()),
        },
    };
    tasks.shutdown().await;
    // Let cancellation of nested connection tasks release their sockets before unlocking.
    tokio::task::yield_now().await;
    drop(guard);
    result
}

async fn forward(listener: TcpListener, host: String, remote: u16) -> io::Result<()> {
    let mut clients = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (mut inbound, _) = accepted?;
                let host = host.clone();
                clients.spawn(async move {
                    let result = async {
                        let mut outbound = timeout(Duration::from_secs(10), TcpStream::connect((host.as_str(), remote))).await??;
                        inbound.set_nodelay(true)?;
                        outbound.set_nodelay(true)?;
                        tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await?;
                        Ok::<_, io::Error>(())
                    }.await;
                    if let Err(error) = result { eprintln!("{host}:{remote}: {error}"); }
                });
            }
            _ = clients.join_next(), if !clients.is_empty() => (),
        }
    }
}

#[cfg(unix)]
fn shutdown_signal() -> io::Result<impl std::future::Future<Output = io::Result<()>>> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let mut hangup = signal(SignalKind::hangup())?;
    Ok(async move {
        tokio::select! {
            _ = interrupt.recv() => (),
            _ = terminate.recv() => (),
            _ = hangup.recv() => (),
        }
        Ok(())
    })
}

#[cfg(windows)]
fn shutdown_signal() -> io::Result<impl std::future::Future<Output = io::Result<()>>> {
    let mut ctrl_c = tokio::signal::windows::ctrl_c()?;
    let mut ctrl_break = tokio::signal::windows::ctrl_break()?;
    Ok(async move {
        tokio::select! {
            _ = ctrl_c.recv() => (),
            _ = ctrl_break.recv() => (),
        }
        Ok(())
    })
}
