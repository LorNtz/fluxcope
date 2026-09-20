"""Atomic preview installation, offline launch, and explicit removal."""
import io
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import socket
import subprocess
import tarfile
import tempfile
import time
import uuid

from .archive import extract_binary
from .model import (BUNDLE, MANIFEST, PreviewError, client_asset, digest,
                    validate_manifest)
from .remote import clean_environment
from .state import atomic_text, checked, private_directory

CLIENT_FILES = {'preview-client', 'gh', 'trusted-root.jsonl', 'licenses.txt'}


def extract_client(archive, destination):
    """The controller bundle contains exactly four regular files, no links."""
    with tarfile.open(archive) as compressed:
        seen = set()
        total = 0
        for member in compressed:
            total += member.size
            if (member.name not in CLIENT_FILES or member.name in seen or not member.isfile()
                    or total > 512 * 1024 * 1024):
                raise PreviewError('Invalid private client bundle.')
            seen.add(member.name)
            with compressed.extractfile(member) as source, (destination / member.name).open('xb') as stream:
                shutil.copyfileobj(source, stream, length=1024 * 1024)
            (destination / member.name).chmod(0o700 if member.name in ('preview-client', 'gh') else 0o600)
        if seen != CLIENT_FILES:
            raise PreviewError('Incomplete private client bundle.')


def installed(state):
    receipt = checked(state.root / 'receipt.json')
    if not receipt.exists():
        return None
    data = json.loads(receipt.read_text())
    name = data.get('generation', '')
    if not re.fullmatch('[0-9a-f]{64}-[0-9a-f]{8}', name):
        raise PreviewError('Malformed preview installation receipt; reinstall this exact preview.')
    directory = checked(state.root / 'installs' / name, directory=True)
    if not directory.is_dir():
        raise PreviewError('Incomplete preview installation; reinstall this exact preview.')
    return directory


def verify_installed(state, directory, downloads):
    for name in (MANIFEST, BUNDLE, f'{state.target}-smoke.json', 'fluxcope'):
        checked(directory / name)
    manifest = json.loads((directory / MANIFEST).read_text())
    if digest(directory / MANIFEST) != directory.name.split('-')[0]:
        raise PreviewError('Saved preview manifest changed; reinstall this exact preview.')
    downloads.verify(directory / MANIFEST, manifest, directory)
    validate_manifest(manifest, {'id': state.id, 'channel': 'preview'})
    if state.target not in manifest['targets']:
        raise PreviewError('Installed preview does not match this host.')
    report = directory / f'{state.target}-smoke.json'
    if digest(report) != manifest['files'][report.name]:
        raise PreviewError('Saved binary verification evidence changed.')
    evidence = json.loads(report.read_text())
    if (evidence.get('target') != state.target or evidence.get('result') != 'passed'
            or evidence.get('source') != manifest['snapshot']
            or evidence.get('version') != manifest['version']
            or digest(directory / 'fluxcope') != evidence['binary_sha256']):
        raise PreviewError('Installed executable differs from signed evidence; reinstall this exact preview.')
    return manifest


def launcher_text(state, directory):
    # Hash the trusted runtime files before executing any downloaded code.
    lines = ['#!/bin/sh', 'set -eu', f'# fluxcope-preview:{state.id}',
             'hash() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi; }']
    for name in sorted(CLIENT_FILES):
        path = directory / name
        lines += [f'p={shlex.quote(str(path))}',
                  '[ ! -L "$p" ] && [ -f "$p" ] || { echo "Preview runtime is missing or unsafe; reinstall." >&2; exit 1; }',
                  f'[ "$(hash "$p" | cut -d " " -f 1)" = "{digest(path)}" ] || {{ echo "Preview runtime changed; reinstall." >&2; exit 1; }}']
    lines.append(f'exec {shlex.quote(str(directory / "preview-client"))} launch {state.id} "$@"')
    return '\n'.join(lines) + '\n'


def remove_tree(path):
    checked(path, directory=True)
    if not path.exists():
        return
    for base, directories, files in os.walk(path, followlinks=False):
        for name in directories:
            checked(Path(base) / name, directory=True)
        for name in files:
            checked(Path(base) / name)
    shutil.rmtree(path)


def install(state, downloads, client_archive=None):
    """Call with the state lock held. Download completely before committing state."""
    checked(state.launcher)
    if state.launcher.exists() and f'# fluxcope-preview:{state.id}\n' not in state.launcher.read_text():
        raise PreviewError('Launcher path is occupied by another file; nothing was overwritten.')
    with tempfile.TemporaryDirectory(prefix='install-', dir=state.root) as temporary:
        staging = Path(temporary)
        manifest, assets = downloads.metadata(state.id, staging)
        if state.target not in manifest['targets']:
            raise PreviewError('This preview does not include this native platform; request its profile or all.')
        report_name = f'{state.target}-smoke.json'
        archive_name = f'fluxcope-{state.target}.tar.xz'
        for name in (report_name, archive_name):
            downloads.payload(manifest, assets, name, staging)
        downloads.verify(staging / archive_name, manifest, staging)
        extract_binary(staging / archive_name, staging / 'fluxcope')
        (staging / archive_name).unlink()
        if manifest.get('format', 1) == 2:
            name = client_asset(state.target)
            if client_archive is None:
                downloads.payload(manifest, assets, name, staging)
                client_archive = staging / name
            if digest(client_archive) != manifest['files'][name]:
                raise PreviewError('Private runtime differs from the signed manifest.')
            extract_client(client_archive, staging)
            if client_archive.parent == staging:
                client_archive.unlink()
        for path in staging.iterdir():
            path.chmod(0o700 if path.name in ('fluxcope', 'preview-client', 'gh') else 0o600)
        generation = digest(staging / MANIFEST) + '-' + uuid.uuid4().hex[:8]
        parent = private_directory(state.root / 'installs')
        destination = parent / generation
        # Validate before exposing a receipt or launcher. The digest prefix is part of verification.
        os.rename(staging, destination)
        try:
            verify_installed(state, destination, downloads)
            if manifest.get('format', 1) == 2:
                checked(state.home / '.local', directory=True)
                checked(state.launcher.parent, directory=True)
                state.launcher.parent.mkdir(mode=0o755, parents=True, exist_ok=True)
                atomic_text(state.launcher, launcher_text(state, destination), 0o700)
            atomic_text(state.root / 'receipt.json', json.dumps({'generation': generation}) + '\n')
        except BaseException:
            # A launcher already committed points to complete verified files; leave those for repair.
            if not state.launcher.exists():
                remove_tree(destination)
            raise
        for old in parent.iterdir():
            if old != destination:
                remove_tree(old)
    print(f'Installed preview {state.id}. It has not been started.')
    if manifest.get('format', 1) == 2:
        print(f'Launch: {shlex.quote(str(state.launcher))}')
        if str(state.launcher.parent) not in os.environ.get('PATH', '').split(os.pathsep):
            print(f'{state.launcher.parent} is not on PATH; use the absolute command above.')
    return destination


def configure_port(state, port=None):
    home = private_directory(state.root / 'home')
    config = private_directory(home / '.fluxcope') / 'config.yml'
    checked(config)
    if port is None and config.exists():
        print(f'Reusing preview port/settings from {config}', flush=True)
        return home
    if not config.exists():
        if port is None:
            with socket.socket() as listener:
                listener.bind(('127.0.0.1', 0))
                port = listener.getsockname()[1]
        if type(port) is not int or not 1 <= port <= 65535:
            raise PreviewError('Preview port must be an integer from 1 to 65535.')
        atomic_text(config, f'server:\n  port: {port}\n')
        print(f'Preview proxy port: {port}\nSettings: {config}', flush=True)
        return home
    from ruamel.yaml import YAML
    yaml = YAML()
    yaml.preserve_quotes = True
    data = yaml.load(config.read_text())
    if not isinstance(data, dict) or not isinstance(data.get('server', {}), dict):
        raise PreviewError(f'Invalid preview settings: {config}')
    if type(port) is not int or not 1 <= port <= 65535:
        raise PreviewError('Preview port must be an integer from 1 to 65535.')
    if port != data.get('server', {}).get('port'):
        data.setdefault('server', {})['port'] = port
        stream = io.StringIO()
        yaml.dump(data, stream)
        atomic_text(config, stream.getvalue())
    print(f'Preview proxy port: {port}\nSettings: {config}', flush=True)
    return home


def run_application(binary, home, environment):
    """Keep the parent's lock until the proxy exits, including launcher termination."""
    process = subprocess.Popen([str(binary)], cwd=home, env=environment)
    previous = {}
    deadline = None
    def forward(number, _frame):
        nonlocal deadline
        if process.poll() is None:
            deadline = time.monotonic() + 5
            process.send_signal(number)
    try:
        for number in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT):
            previous[number] = signal.signal(number, forward)
        while True:
            try:
                return process.wait(timeout=0.5)
            except subprocess.TimeoutExpired:
                if deadline is not None and time.monotonic() >= deadline:
                    process.kill()
                    return process.wait()
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        for number, handler in previous.items():
            signal.signal(number, handler)


def launch(state, downloads, port=None):
    directory = installed(state)
    if directory is None:
        raise PreviewError('Preview is not installed; run its version-specific installer.')
    manifest = verify_installed(state, directory, downloads)
    home = configure_port(state, port)
    environment = clean_environment(home)
    environment.update(XDG_CONFIG_HOME=str(home / '.config'), XDG_DATA_HOME=str(home / '.local/share'),
                       XDG_CACHE_HOME=str(home / '.cache'))
    print(f"Running preview {state.id} at {manifest['source']} (isolated state).", flush=True)
    if run_application(directory / 'fluxcope', home, environment):
        raise PreviewError('Preview exited unsuccessfully. If its port is occupied, retry with --port PORT.')


def uninstall(state, purge=False):
    checked(state.launcher)
    if purge:
        try:
            with open('/dev/tty', 'r+') as terminal:
                terminal.write(f'Delete all settings and certificates for preview {state.id}? Type {state.id}: ')
                terminal.flush()
                answer = terminal.readline().strip()
        except OSError as error:
            raise PreviewError('Purging requires an interactive terminal.') from error
        if answer != str(state.id):
            raise PreviewError('Purge cancelled; nothing was removed.')
    if state.launcher.exists():
        if f'# fluxcope-preview:{state.id}\n' not in state.launcher.read_text():
            raise PreviewError('Refusing to remove an unrelated launcher.')
        state.launcher.unlink()
    remove_tree(state.root / 'installs')
    checked(state.root / 'receipt.json').unlink(missing_ok=True)
    # Remove a legacy just-preview-run executable too; preserve its HOME.
    remove_tree(state.root / 'bin')
    if purge:
        remove_tree(state.root)
        print(f'Removed preview {state.id} and its state.')
    else:
        print(f'Removed preview {state.id}. Settings retained at: {state.root / "home"}')
        print('The launcher is gone. Reinstall this ID while available to reuse these settings.')
