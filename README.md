# prox

tiny tcp forwarder for windows, mac and linux. pick a host, pick some ports, done.

## install

[grab a build](https://github.com/velddev/prox/releases/latest) and run the installer
from the folder you downloaded it to:

```powershell
.\prox.exe install
```

on mac/linux, unpack the archive and run `./prox install`.
it copies itself into a standard per-user location and sets up PATH. no admin needed.
reopen your terminal afterwards. rerunning it won't duplicate PATH entries.

## just run it

```text
prox
```

pick a saved environment or create one. choose a number or type its name.
creating one asks for a name, host and ports, then saves it and starts forwarding.
`q` cancels without saving a partial environment. Ctrl+C stops the proxy.

## or use flags

```text
prox -p 3000,8080,25565 --host example.com -d
```

`-d` is daemon mode. it keeps running after you close the terminal.
leave it off to run in the foreground.

remap a port with `LOCAL:REMOTE`:

```text
prox -p 3000:8080,25565 --host example.com -d
```

that sends local port **3000** to remote port **8080**. regular ports and mappings can be mixed.
there's no default host: use `--host`, a saved environment, or the `PROX_HOST` environment variable.

## save an environment

```text
prox env add dev
prox env dev host set example.com
prox env dev ports set 3000,8080,80
prox env dev get
prox -e dev -d
```

saved ports support `LOCAL:REMOTE` too. `--host` and `-p` override saved values for
one run. `PROX_HOST` is the fallback when neither a flag nor the environment has a host.

```text
prox env list
prox env dev host clear
prox env dev ports clear
prox env remove dev
```

`clear` removes a field. `remove` deletes the environment.
changes apply next time you start it, not to a running proxy.
names can use letters, numbers, `_` and `-`; start with a letter or number.
`add`, `remove`, `list` and `help` are reserved. max 64 characters.

## manage it

```text
prox status
prox stop
```

one instance per user. stop it before switching ports or environments.
if a requested port is taken, startup fails without leaving a partial proxy running.
stopping closes the connections and frees the ports. daemon mode doesn't start again after a reboot.

## a few details

- tcp only. listeners bind to `127.0.0.1`; the destination must be reachable from your machine.
- http, websockets and other tcp traffic pass through as-is. URLs and OAuth redirects don't change.
- environments stay in local config. `PROX_CONFIG_DIR` overrides its directory.
- state and daemon connection errors (`prox.log`) stay local too. `PROX_STATE_DIR` overrides their directory.
- bare `prox` shows help when input or output is redirected.

| | windows | mac/linux |
| --- | --- | --- |
| config | `%LOCALAPPDATA%\prox` | `${XDG_CONFIG_HOME:-~/.config}/prox` |
| state / logs | `%LOCALAPPDATA%\prox` | `${XDG_STATE_HOME:-~/.local/state}/prox` |

`prox install` uses `%LOCALAPPDATA%\Programs\prox` on windows,
`~/Applications/prox/bin` on mac, and `~/.local/bin` on linux.
on mac/linux it sets up PATH for bash, zsh, fish, sh, dash or ksh.

## build it

rust 1.89+:

```text
cargo install --path . --locked
```

or build and run the checks:

```text
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
python3 tests/smoke.py target/release/prox
python3 tests/install.py target/release/prox
python3 tests/environments.py target/release/prox
```

on windows, use `python` and `target/release/prox.exe` for the python checks.
CI builds and tests all three platforms.
