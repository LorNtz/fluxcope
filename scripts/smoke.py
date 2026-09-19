#!/usr/bin/env python3
"""Exercise an explicit packaged binary with isolated HOME, fixtures and a real PTY."""
from __future__ import annotations

import argparse
import contextlib
import fcntl
import hashlib
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import platform
import pty
import re
import select
import socket
import ssl
import struct
import subprocess
import tempfile
import termios
import threading
import time


class Terminal:
    def __init__(self, binary: Path, home: Path):
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 120, 0, 0))
        self.original = termios.tcgetattr(self.slave)
        self.output = bytearray()
        self.process = subprocess.Popen([str(binary)], cwd=home,
            env={**os.environ, 'HOME': str(home), 'TERM': 'xterm-256color'},
            stdin=self.slave, stdout=self.slave, stderr=self.slave)

    def drain(self, timeout: float = 0.1):
        if select.select([self.master], [], [], timeout)[0]:
            data = os.read(self.master, 65536)
            self.output.extend(data)
            # Crossterm may query cursor position during terminal setup.
            if b'\x1b[6n' in data:
                os.write(self.master, b'\x1b[1;1R')

    def wait(self, predicate, message: str, timeout: float = 20):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.drain()
            if predicate():
                return
            if self.process.poll() is not None:
                raise AssertionError(f'{message}: app exited {self.process.returncode}; {self.text()[-1500:]}')
        raise AssertionError(f'{message}: timed out; {self.text()[-1500:]}')

    def text(self):
        return re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]', '', self.output.decode(errors='replace'))

    def key(self, value: bytes):
        os.write(self.master, value)
        # A readable output buffer makes select return immediately. Keep the
        # intended inter-key interval while draining so Esc + the next key cannot
        # coalesce into an Alt sequence under a burst of terminal redraws.
        deadline = time.monotonic() + 0.2
        while (remaining := deadline - time.monotonic()) > 0:
            self.drain(min(remaining, 0.05))

    def finish(self, expected=0):
        self.wait(lambda: self.process.poll() is not None, 'app shutdown')
        assert self.process.returncode == expected, self.text()[-1500:]
        assert termios.tcgetattr(self.slave) == self.original, 'terminal attributes were not restored'
        if expected == 0:
            assert b'\x1b[?1049l' in self.output, 'alternate screen was not restored'

    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        os.close(self.master)
        os.close(self.slave)

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()


class Origin(BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps({'path': self.path, 'fixture': 'fluxcope-smoke'}).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


@contextlib.contextmanager
def fixture_server():
    server = ThreadingHTTPServer(('127.0.0.1', 0), Origin)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield server.server_port
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def free_port():
    with socket.socket() as connection:
        connection.bind(('127.0.0.1', 0))
        return connection.getsockname()[1]


def ready(port):
    try:
        with socket.create_connection(('127.0.0.1', port), timeout=0.1):
            return True
    except OSError:
        return False


def request(port, url, *, ca: Path | None = None):
    if ca:
        context = ssl.create_default_context(cafile=str(ca))
        client = http.client.HTTPSConnection('127.0.0.1', port, context=context, timeout=10)
        client.set_tunnel('mapped.example', 443)
    else:
        client = http.client.HTTPConnection('127.0.0.1', port, timeout=10)
    try:
        client.request('GET', url)
        response = client.getresponse()
        return response.status, response.read()
    finally:
        client.close()


def check_permissions(home: Path):
    for path in (home / '.fluxcope').rglob('*'):
        assert not path.is_symlink(), f'unexpected state symlink: {path.name}'
        assert path.stat().st_mode & 0o777 == (0o700 if path.is_dir() else 0o600), path.name
    assert (home / '.fluxcope').stat().st_mode & 0o777 == 0o700


def smoke_log(state: Path) -> Path:
    # The isolated smoke HOME contains one app instance, using either layout.
    logs = [path for path in (state / 'fluxcope.log', *(state / 'logs').glob('*.log'))
            if path.is_file()]
    assert len(logs) == 1, f'expected exactly one app log in {state}, found {len(logs)}'
    return logs[0]


def smoke(binary: Path, version: str):
    with tempfile.TemporaryDirectory(prefix='fluxcope-smoke-') as directory:
        home = Path(directory)
        for option in ('--help', '--version'):
            result = subprocess.run([str(binary), option], cwd=home,
                env={**os.environ, 'HOME': str(home)}, capture_output=True, timeout=10, check=True)
            assert b'fluxcope' in result.stdout.lower(), option
            if option == '--version':
                assert result.stdout.decode().strip() == f'fluxcope {version}'
            assert not list(home.iterdir()), f'{option} created local state'
        with fixture_server() as origin:
            state = home / '.fluxcope'
            state.mkdir(mode=0o700)
            local = home / 'mapped.txt'
            local.write_text('local mapping fixture\n')
            proxy = free_port()
            config = {'server': {'port': proxy}, 'proxy': {'active_preset': 'smoke', 'presets': [{
                'name': 'smoke',
                'map_remote': {'rules': [{'from': 'http://origin.example', 'to': f'http://127.0.0.1:{origin}'}]},
                'map_local': {'rules': [{'from': 'https://mapped.example/data', 'to': str(local)}]},
            }]}}
            (state / 'config.yml').write_text(json.dumps(config))
            (state / 'config.yml').chmod(0o600)
            with Terminal(binary, home) as terminal:
                terminal.wait(lambda: ready(proxy) and b'\x1b[?1049h' in terminal.output, 'proxy/TUI startup')
                status, body = request(proxy, f'http://127.0.0.1:{origin}/forwarded?test=1')
                assert status == 200 and json.loads(body)['path'] == '/forwarded?test=1'
                status, body = request(proxy, 'http://origin.example/remapped')
                assert status == 200 and json.loads(body)['path'] == '/remapped'
                ca = state / 'certificate' / 'fluxcope-ca.pem'
                terminal.wait(ca.exists, 'public CA PEM creation')
                status, body = request(proxy, '/data', ca=ca)
                assert status == 200 and body == local.read_bytes(), 'HTTPS MITM / map-local'
                terminal.wait(lambda: 'mapped.example' in terminal.text(), 'captured request visible in TUI')
                # Recording controls must not interrupt forwarding or mapping.
                terminal.key(b'r')
                status, body = request(proxy, 'http://origin.example/recording-off')
                assert status == 200 and json.loads(body)['path'] == '/recording-off'
                terminal.key(b'r')
                terminal.key(b'c')
                terminal.key(b'\x1b')
                terminal.key(b'@')
                terminal.key(b'@')
                terminal.key(b'm')
                terminal.key(b'\x1b')
                terminal.key(b'q')
                terminal.finish()
            certificate_digest = hashlib.sha256(ca.read_bytes()).hexdigest()
            log_path = smoke_log(state)
            log = log_path.read_text()
            match = re.search(r'CA certificate download URL: http://[^:/]+:(\d+)/fluxcope-ca.pem', log)
            assert match, 'CA download URL missing'
            # Restart checks persistence and the restricted CA download endpoint.
            with Terminal(binary, home) as terminal:
                terminal.wait(lambda: ready(proxy) and b'\x1b[?1049h' in terminal.output, 'restart')
                terminal.wait(lambda: len(re.findall(r'CA certificate download URL:', log_path.read_text())) >= 2,
                              'restart CA service')
                matches = re.findall(r'CA certificate download URL: http://[^:/]+:(\d+)/fluxcope-ca.pem', log_path.read_text())
                download_port = int(matches[-1])
                status, pem = request(download_port, '/fluxcope-ca.pem')
                assert status == 200 and pem == ca.read_bytes()
                for forbidden in ('/ca_key.der', '/../ca_key.der', '/config.yml'):
                    status, _ = request(download_port, forbidden)
                    assert status == 404, forbidden
                assert hashlib.sha256(ca.read_bytes()).hexdigest() == certificate_digest, 'CA changed on restart'
                terminal.key(b'q')
                terminal.finish()
            check_permissions(home)
            assert not (home / 'debug.log').exists()
            # Occupied-port startup must fail before entering raw/alternate-screen mode.
            with socket.socket() as occupied:
                occupied.bind(('0.0.0.0', proxy))
                occupied.listen()
                with Terminal(binary, home) as terminal:
                    terminal.wait(lambda: terminal.process.poll() is not None, 'occupied-port rejection')
                    assert terminal.process.returncode != 0
                    assert termios.tcgetattr(terminal.slave) == terminal.original
                    assert b'\x1b[?1049h' not in terminal.output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--version', required=True)
    parser.add_argument('--target')
    parser.add_argument('--require-macos', type=int)
    parser.add_argument('--report', type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    machine = platform.machine()
    if args.target:
        architecture = 'aarch64' if machine == 'arm64' else machine
        assert args.target.startswith(architecture + '-'), f'native architecture mismatch: {machine}'
        assert ('apple-darwin' in args.target) == (platform.system() == 'Darwin')
        if platform.system() == 'Darwin':
            translated = subprocess.run(['sysctl', '-in', 'sysctl.proc_translated'], capture_output=True, text=True)
            assert translated.stdout.strip() != '1', 'Rosetta is not native Intel validation'
    if args.require_macos:
        assert platform.system() == 'Darwin' and int(platform.mac_ver()[0].split('.')[0]) == args.require_macos, f'macOS {args.require_macos} is required'
    smoke(binary, args.version)
    report = {'schema': 1, 'version': args.version, 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
              'target': args.target, 'system': platform.system(), 'machine': machine,
              'os_version': platform.mac_ver()[0] if platform.system() == 'Darwin' else platform.release(),
              'result': 'passed', 'checks': ['help-version-no-state', 'http-forward', 'map-remote', 'https-mitm-map-local',
              'recording-forward', 'ca-persistence-download', 'private-state', 'pty-shutdown', 'occupied-port']}
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
