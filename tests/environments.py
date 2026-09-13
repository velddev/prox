"""Saved environment persistence, launch overrides, and interactive creation."""
import concurrent.futures
import json
import os
from pathlib import Path
import signal
import socket
import socketserver
import subprocess
import sys
import tempfile
import threading
import time

EXE = str(Path(sys.argv[1]).resolve())


class Echo(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.sendall(self.request.recv(1024))


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def transfer(port):
    with socket.create_connection(("127.0.0.1", port), timeout=3) as connection:
        connection.sendall(b"profile works")
        received = bytearray()
        while len(received) < 13:
            data = connection.recv(13 - len(received))
            assert data
            received.extend(data)
        assert received == b"profile works"


with tempfile.TemporaryDirectory(prefix="prox-env-") as directory:
    root = Path(directory)
    env = dict(os.environ, PROX_CONFIG_DIR=str(root / "config"), PROX_STATE_DIR=str(root / "state"))
    env.pop("PROX_HOST", None)
    config = root / "config/environments.json"

    def run(*args, ok=True, extra=None):
        result = subprocess.run([EXE, *args], env=env | (extra or {}), capture_output=True,
                                text=True, encoding="utf-8", timeout=15)
        assert (result.returncode == 0) == ok, (args, result.stdout, result.stderr)
        return result.stdout + result.stderr

    assert "Usage:" in run()  # No hanging prompt when redirected.
    assert "No saved environments" in run("env")
    run("env", "add", "dev")
    assert "(not set)" in run("env", "dev", "get")
    run("env", "dev", "host", "set", "example.com")
    run("env", "dev", "ports", "set", "3000,8080:80")
    assert "host: example.com" in run("env", "dev", "get")
    assert "ports: 3000,8080:80" in run("env", "dev", "get")
    before = config.read_bytes()
    for args in [("add", "dev"), ("add", "../escape"), ("add", "list"),
                 ("dev", "ports", "set", "3000,3000"), ("dev", "ports", "set", "0"),
                 ("dev", "host", "set", "https://example.com"), ("missing", "get"),
                 ("missing", "host", "clear"), ("remove", "missing")]:
        run("env", *args, ok=False)
        assert config.read_bytes() == before
    run("env", "dev", "host", "clear")
    run("env", "dev", "host", "clear")
    assert "host: (not set)" in run("env", "dev", "get")
    assert "missing destination" in run("-e", "dev", "-d", ok=False)
    run("env", "dev", "ports", "clear")
    assert "missing ports" in run("-e", "dev", "--host", "127.0.0.1", "-d", ok=False)
    if os.name != "nt":
        assert config.stat().st_mode & 0o777 == 0o600
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        list(pool.map(lambda i: run("env", "add", f"parallel-{i}"), range(16)))
    assert len(json.loads(config.read_text())["environments"]) == 17
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        jobs = [pool.submit(run, "env", "dev", "host", "set", "127.0.0.1"),
                pool.submit(run, "env", "dev", "ports", "set", "3000:8080")]
        for job in jobs:
            job.result()
    saved = json.loads(config.read_text())["environments"]["dev"]
    assert saved["host"] == "127.0.0.1" and saved["ports"][0]["remote"] == 8080
    print("PASS: CRUD, validation, clear, persistence, permissions and concurrent updates")

    server = socketserver.ThreadingTCPServer(("127.0.0.1", 0), Echo)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        remote = server.server_address[1]
        local = free_port()
        run("env", "dev", "ports", "set", f"{local}:{remote}")
        run("-e", "dev", "-d", extra={"PROX_HOST": "unused.invalid"})
        transfer(local)
        run("env", "dev", "host", "set", "unused.invalid")
        transfer(local)  # Saved edits cannot change a running daemon.
        run("stop")
        other = free_port()
        before = config.read_bytes()
        run("-e", "dev", "--host", "127.0.0.1", "-p", f"{other}:{remote}", "-d")
        transfer(other)
        assert config.read_bytes() == before
        run("stop")
        run("env", "dev", "host", "clear")
        run("-e", "dev", "-d", extra={"PROX_HOST": "127.0.0.1"})
        transfer(local)
        run("env", "remove", "dev")
        transfer(local)
        run("env", "dev", "get", ok=False)
        run("stop")
        print("PASS: saved launches, host/port overrides, PROX_HOST fallback and runtime isolation")

        if os.name != "nt":
            import pty
            import select

            interactive = dict(env, PROX_CONFIG_DIR=str(root / "interactive"),
                               PROX_STATE_DIR=str(root / "interactive-state"))
            interactive_file = root / "interactive/environments.json"

            class Terminal:
                def __init__(self):
                    self.master, slave = pty.openpty()
                    self.process = subprocess.Popen([EXE], env=interactive, stdin=slave, stdout=slave,
                                                    stderr=slave, start_new_session=True)
                    os.close(slave)
                    self.output = b""

                def expect(self, text):
                    deadline = time.monotonic() + 10
                    while text.encode() not in self.output:
                        assert time.monotonic() < deadline, self.output
                        if select.select([self.master], [], [], 0.1)[0]:
                            self.output += os.read(self.master, 8192)
                    self.output = self.output.split(text.encode(), 1)[1]

                def send(self, text):
                    os.write(self.master, (text + "\n").encode())

                def close(self, stop=False):
                    try:
                        if stop:
                            self.process.send_signal(signal.SIGINT)
                        assert self.process.wait(timeout=5) == 0, self.output
                    finally:
                        if self.process.poll() is None:
                            self.process.kill()
                            self.process.wait(timeout=5)
                        os.close(self.master)

            terminal = Terminal()
            try:
                terminal.expect("Name: ")
                terminal.send("cancelled")
                terminal.expect("Host: ")
                terminal.send("q")
            finally:
                terminal.close()
            assert not interactive_file.exists()

            terminal = Terminal()
            try:
                terminal.expect("Name: ")
                terminal.send("../invalid")
                terminal.expect("Name: ")
                terminal.send("dev")
                terminal.expect("Host: ")
                terminal.send("127.0.0.1")
                terminal.expect("comma-separated): ")
                terminal.send("0")
                terminal.expect("comma-separated): ")
                terminal.send(f"{local}:{remote}")
                terminal.expect("Ctrl+C to stop.")
                transfer(local)
            finally:
                terminal.close(stop=True)
            assert "dev" in json.loads(interactive_file.read_text())["environments"]

            terminal = Terminal()
            try:
                terminal.expect("q to quit): ")
                terminal.send("dev")
                terminal.expect("Ctrl+C to stop.")
                transfer(local)
            finally:
                terminal.close(stop=True)

            terminal = Terminal()
            try:
                terminal.expect("q to quit): ")
                terminal.send("2")  # Create a second environment from the picker.
                terminal.expect("Name: ")
                terminal.send("staging")
                terminal.expect("Host: ")
                terminal.send("127.0.0.1")
                terminal.expect("comma-separated): ")
                terminal.send(f"{other}:{remote}")
                terminal.expect("Ctrl+C to stop.")
                transfer(other)
            finally:
                terminal.close(stop=True)
            assert len(json.loads(interactive_file.read_text())["environments"]) == 2
            print("PASS: real terminal picker, creation, validation, cancellation and foreground forwarding")
    finally:
        run("stop")
        server.shutdown()
        server.server_close()

    config.write_text("{broken json")
    run("env", "add", "do-not-overwrite", ok=False)
    assert config.read_text() == "{broken json"
    print("PASS: corrupt config is never overwritten")
