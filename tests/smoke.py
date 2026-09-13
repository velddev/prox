"""Black-box TCP and process-lifecycle checks, using only Python's standard library."""
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
        try:
            self.request.sendall(self.server.marker)
            while data := self.request.recv(65536):
                self.request.sendall(data)
            # A response after the client's EOF exercises TCP half-close handling.
            self.request.sendall(b"finished")
        except (ConnectionError, OSError):
            pass


class Server(socketserver.ThreadingTCPServer):
    daemon_threads = True


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def receive(sock, size):
    result = bytearray()
    while len(result) < size:
        data = sock.recv(size - len(result))
        assert data, "unexpected EOF"
        result.extend(data)
    return bytes(result)


def assert_closed(ports):
    for port in ports:
        with socket.socket() as sock:
            sock.settimeout(1)
            assert sock.connect_ex(("127.0.0.1", port)) != 0, f"{port} still listens"


with tempfile.TemporaryDirectory(prefix="prox-test-") as directory:
    env = dict(os.environ, PROX_STATE_DIR=directory)
    env.pop("PROX_HOST", None)
    servers = []
    foreground = None
    held = None

    def run(*args, ok=True):
        result = subprocess.run([EXE, *args], env=env, capture_output=True, text=True,
                                encoding="utf-8", timeout=15)
        assert (result.returncode == 0) == ok, (args, result.stdout, result.stderr)
        return result.stdout + result.stderr

    try:
        for marker in (b"first", b"second"):
            server = Server(("127.0.0.1", 0), Echo)
            server.marker = marker
            threading.Thread(target=server.serve_forever, daemon=True).start()
            servers.append(server)
        locals_ = [free_port(), free_port()]
        assert locals_[0] != locals_[1]
        mappings = ",".join(f"{local}:{server.server_address[1]}"
                            for local, server in zip(locals_, servers))
        args = ["-p", mappings, "--host", "127.0.0.1"]

        assert "not running" in run("status")
        assert "missing destination" in run("-p", "3000", ok=False)
        for invalid in ("0", "70000", "3000,3000", "3000:", "1:2:3"):
            run("-p", invalid, ok=False)

        assert "Daemon started" in run(*args, "-d")
        assert "running" in run("status")
        assert "already running" in run(*args, "-d", ok=False)

        def transfer(index):
            target = index % 2
            with socket.create_connection(("127.0.0.1", locals_[target]), timeout=10) as conn:
                marker = servers[target].marker
                assert receive(conn, len(marker)) == marker
                conn.sendall(b"interactive")
                assert receive(conn, 11) == b"interactive"
                payload = (f"stream-{index}-" * 20000).encode()
                conn.sendall(payload)
                conn.shutdown(socket.SHUT_WR)
                response = bytearray()
                while data := conn.recv(65536):
                    response.extend(data)
                assert response == payload + b"finished"

        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
            list(pool.map(transfer, range(16)))
        print("PASS: daemon survives launcher exit, multiple mappings, 16 concurrent streams, half-close")

        state = json.loads(Path(directory, "state.json").read_text())
        with socket.create_connection(("127.0.0.1", state["control_port"]), timeout=3) as conn:
            conn.sendall(b"wrong-token stop\n")
            assert conn.recv(20) == b""
        assert "running" in run("status")
        held = socket.create_connection(("127.0.0.1", locals_[0]), timeout=3)
        assert receive(held, 5) == b"first"
        assert "stopped" in run("stop")
        assert held.recv(1) == b""
        held.close()
        held = None
        assert_closed(locals_)
        assert "not running" in run("stop")
        print("PASS: unauthenticated control rejected; stop releases listeners and active sockets")

        with socket.socket() as occupied:
            occupied.bind(("127.0.0.1", 0))
            occupied.listen()
            unused = free_port()
            conflict = f"{unused}:{servers[0].server_address[1]},{occupied.getsockname()[1]}"
            assert "cannot listen" in run("-p", conflict, "--host", "127.0.0.1", "-d", ok=False)
            assert_closed([unused])
        print("PASS: occupied-port failure is reported without partial listeners")

        foreground = subprocess.Popen([EXE, *args], env=env, stdout=subprocess.PIPE,
                                      stderr=subprocess.PIPE, text=True, encoding="utf-8")
        for _ in range(100):
            if Path(directory, "state.json").exists():
                break
            assert foreground.poll() is None
            time.sleep(0.05)
        assert "running" in run("status")
        if os.name != "nt":
            foreground.send_signal(signal.SIGINT)
            foreground.communicate(timeout=5)
            assert foreground.returncode == 0
            assert_closed(locals_)
            foreground = subprocess.Popen([EXE, *args], env=env, stdout=subprocess.PIPE,
                                          stderr=subprocess.PIPE)
            for _ in range(100):
                if Path(directory, "state.json").exists():
                    break
                time.sleep(0.05)
        # A forced crash leaves stale state, which must not block a fresh start.
        foreground.kill()
        foreground.communicate(timeout=5)
        foreground = None
        assert "not running" in run("status")
        assert "Daemon started" in run(*args, "-d")
        assert "stopped" in run("stop")
        assert_closed(locals_)
        print("PASS: foreground termination and recovery after a forced crash")
    finally:
        if foreground and foreground.poll() is None:
            foreground.kill()
            foreground.communicate(timeout=5)
        if held:
            held.close()
        subprocess.run([EXE, "stop"], env=env, capture_output=True, timeout=10)
        for server in servers:
            server.shutdown()
            server.server_close()
