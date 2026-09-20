#!/usr/bin/env python3
"""Run the exact signed installer through a local HTTPS replay before publication.

The replay substitutes transport only. The unmodified shell, frozen client,
checksums, Sigstore verification and controller identity checks all run normally.
No fixture endpoint or signature bypass is built into the shipped client.
"""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import pty
import select
import signal
import shlex
import ssl
import subprocess
import tempfile
import threading
import time
from unittest.mock import patch

from preview_client.model import INSTALLER, MANIFEST, REPO, client_asset, digest
from preview_client.remote import clean_environment
from preview_client.state import State, host_target
from smoke import Terminal, free_port, ready, request, fixture_server


class Replay(BaseHTTPRequestHandler):
    def do_CONNECT(self):
        if self.path not in ('api.github.com:443', 'github.com:443'):
            self.send_error(403)
            return
        self.send_response(200, 'Connection established')
        self.end_headers()
        self.connection = self.server.context.wrap_socket(self.connection, server_side=True)
        self.rfile = self.connection.makefile('rb')
        self.wfile = self.connection.makefile('wb')
        self.handle_one_request()

    def do_GET(self):
        server = self.server
        manifest = server.manifest
        prefix = f'/repos/{REPO}/'
        path = self.path.removeprefix(prefix)
        controller = manifest['controller']
        responses = {
            f"actions/runs/{manifest['id']}": {
                'id': manifest['id'], 'repository': {'full_name': REPO}, 'event': 'workflow_dispatch',
                'head_branch': 'master', 'workflow_id': 1, 'path': '.github/workflows/preview.yml',
                'head_sha': controller, 'actor': {'login': manifest['actor']}, 'created_at': manifest['created_at'],
                'display_title': f"Preview #{manifest['pr']} {manifest['source']} {manifest['profile']}"},
            'actions/workflows/preview.yml': {'id': 1},
            f'compare/{controller}...master': {'status': 'identical'},
            f"git/ref/tags/{manifest['tag']}": {'object': {'type': 'commit', 'sha': manifest['snapshot']}},
            f"git/commits/{manifest['snapshot']}": {'parents': [{'sha': manifest['source']}], 'tree': {'sha': manifest['tree']}},
            f"releases/tags/{manifest['tag']}": {
                'draft': False, 'prerelease': True, 'immutable': True, 'assets': server.assets,
                'published_at': '2020-01-01T00:00:00Z' if server.expired else datetime.now(timezone.utc).isoformat()},
        }
        server.requests.append(self.path)
        if self.path.startswith(prefix) and path in responses:
            body = json.dumps(responses[path]).encode()
            self.send_response(200)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        download = f"/{REPO}/releases/download/{manifest['tag']}/"
        name = self.path.removeprefix(download)
        if not self.path.startswith(download) or name not in {a['name'] for a in server.assets}:
            self.send_error(404)
            return
        source = server.directory / name
        self.send_response(200)
        self.send_header('Content-Length', str(source.stat().st_size))
        self.end_headers()
        with source.open('rb') as stream:
            first = True
            while chunk := stream.read(1024 * 1024):
                if first and name == server.corrupt:
                    chunk = bytes([chunk[0] ^ 1]) + chunk[1:]
                self.wfile.write(chunk)
                first = False

    def log_message(self, *_):
        pass


@contextmanager
def replay(directory, temporary):
    certificate = temporary / 'replay.pem'
    key = temporary / 'replay.key'
    subprocess.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
                    '-keyout', str(key), '-out', str(certificate), '-subj', '/CN=github.com',
                    '-addext', 'subjectAltName=DNS:github.com,DNS:api.github.com'],
                   check=True, capture_output=True)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(certificate, key)
    server = ThreadingHTTPServer(('127.0.0.1', 0), Replay)
    server.daemon_threads = True
    server.context = context
    server.directory = directory
    server.manifest = json.loads((directory / MANIFEST).read_text())
    server.assets = [{'name': p.name, 'size': p.stat().st_size, 'digest': 'sha256:' + digest(p)}
                     for p in directory.iterdir() if p.is_file() and not p.name.endswith('-install.json')]
    server.requests = []
    server.expired = False
    server.corrupt = None
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield server, certificate
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def command(args, environment, *, error=None):
    result = subprocess.run(args, env=environment, cwd=environment['HOME'], text=True, capture_output=True, timeout=120)
    text = result.stdout + result.stderr
    if error:
        assert result.returncode != 0 and error in text, text
    else:
        assert result.returncode == 0, text
    return text


def purge(launcher, identifier, environment):
    pid, descriptor = pty.fork()
    if pid == 0:
        os.execve(launcher, [str(launcher), '--uninstall', '--purge'], environment)
    output = bytearray()
    sent = False
    closed = False
    try:
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            if closed:
                time.sleep(0.05)
            elif select.select([descriptor], [], [], 0.1)[0]:
                try:
                    chunk = os.read(descriptor, 65536)
                    output.extend(chunk)
                    closed = not chunk
                except OSError:
                    closed = True
            if not sent and b'Type ' in output:
                os.write(descriptor, f'{identifier}\n'.encode())
                sent = True
            done, status = os.waitpid(pid, os.WNOHANG)
            if done:
                assert os.waitstatus_to_exitcode(status) == 0 and sent, output.decode(errors='replace')
                return
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
        raise AssertionError('Purge did not complete: ' + output.decode(errors='replace'))
    finally:
        os.close(descriptor)


def check(directory, target, report):
    assert target == host_target(), 'installation must execute on its native platform'
    manifest = json.loads((directory / MANIFEST).read_text())
    with tempfile.TemporaryDirectory(prefix='preview-install-check-') as temporary:
        temporary = Path(temporary).resolve()
        home = temporary / 'home'
        home.mkdir(mode=0o700)
        environment = clean_environment(home)
        environment.update(PATH='/usr/bin:/bin:/usr/sbin:/sbin', PYTHONPATH='', PYTHONHOME='', NO_PROXY='', no_proxy='')
        with replay(directory, temporary) as (server, certificate):
            proxy = f'http://127.0.0.1:{server.server_port}'
            for name in ('http_proxy', 'https_proxy', 'HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'all_proxy'):
                environment[name] = proxy
            environment.update(SSL_CERT_FILE=str(certificate), CURL_CA_BUNDLE=str(certificate))
            installer = ['sh', str(directory / INSTALLER)]
            state = State(manifest['id'], target, home)
            server.corrupt = client_asset(target)
            command(installer, environment, error='checksum mismatch')
            assert not state.launcher.exists()
            server.corrupt = None
            command(installer, environment)
            assert not (state.root / 'home').exists(), 'install must not start app or create settings'
            assert not (home / '.fluxcope').exists()
            # Fail early for an unsupported architecture, without downloading anything.
            fakebin = temporary / 'fakebin'
            fakebin.mkdir()
            fakeuname = fakebin / 'uname'
            fakeuname.write_text('#!/bin/sh\nif [ "$1" = -m ]; then echo unsupported; else echo Linux; fi\n')
            fakeuname.chmod(0o755)
            before = len(server.requests)
            command(installer, {**environment, 'PATH': str(fakebin) + ':' + environment['PATH']}, error='Unsupported architecture')
            assert len(server.requests) == before
            # Initial launch chooses and persists a port. Repeat launch uses it offline.
            offline = {**environment, 'http_proxy': 'http://127.0.0.1:1', 'https_proxy': 'http://127.0.0.1:1',
                       'HTTP_PROXY': 'http://127.0.0.1:1', 'HTTPS_PROXY': 'http://127.0.0.1:1'}
            config = state.root / 'home/.fluxcope/config.yml'
            with patch.dict(os.environ, offline, clear=True), Terminal(state.launcher, home) as terminal:
                terminal.wait(lambda: config.exists(), 'first-launch settings')
                import re
                port = int(re.search(r'port:\s*(\d+)', config.read_text())[1])
                terminal.wait(lambda: ready(port), 'offline proxy startup', timeout=60)
                with fixture_server() as origin:
                    assert request(port, f'http://127.0.0.1:{origin}/preview-installer')[0] == 200
                terminal.key(b'q')
                terminal.finish()
            original = config.read_bytes()
            command(installer, environment)
            assert config.read_bytes() == original, 'reinstall changed preview settings'
            server.expired = True
            command(installer, environment, error='expired')
            # The app rewrites YAML on startup; check settings, not comment/quote
            # formatting. The port editor's round-trip formatting has a unit test.
            config.write_text(f'# preview settings\nserver:\n  port: {port}\n'
                              'recording:\n  start_record_on_launch: false\n'
                              'ui:\n  request_list:\n    auto_expand: true\n')
            new_port = free_port()
            while new_port == port:
                new_port = free_port()
            wrapper = temporary / 'launch'
            wrapper.write_text(f'#!/bin/sh\nexec {shlex.quote(str(state.launcher))} --port {new_port}\n')
            wrapper.chmod(0o755)
            before = len(server.requests)
            with patch.dict(os.environ, offline, clear=True), Terminal(wrapper, home) as terminal:
                terminal.wait(lambda: ready(new_port), 'offline launch after expiry', timeout=60)
                saved = config.read_text()
                assert 'start_record_on_launch: false' in saved and 'auto_expand: true' in saved, saved
                terminal.key(b'q')
                terminal.finish()
            assert len(server.requests) == before, 'offline launch made network requests'
            # Altered executable must be rejected before application startup.
            receipt = json.loads((state.root / 'receipt.json').read_text())
            binary = state.root / 'installs' / receipt['generation'] / 'fluxcope'
            with binary.open('ab') as stream:
                stream.write(b'corrupt')
            command([str(state.launcher)], offline, error='differs from signed evidence')
            command([str(state.launcher), '--uninstall'], offline)
            assert not state.launcher.exists() and config.exists()
            server.expired = False
            command(installer, environment)
            purge(state.launcher, state.id, offline)
            assert not state.launcher.exists() and not state.root.exists()
            assert not (home / '.fluxcope').exists()
    report.write_text(json.dumps({'schema': 1, 'id': manifest['id'], 'controller': manifest['controller'],
                                 'target': target, 'manifest_sha256': digest(directory / MANIFEST), 'result': 'passed'}) + '\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', required=True)
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    check(args.directory, args.target, args.report)
