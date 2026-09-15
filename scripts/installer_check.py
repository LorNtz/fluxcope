#!/usr/bin/env python3
"""Exercise the generated installer against real and corrupted local downloads."""
import argparse
from functools import partial
import http.server
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import threading

from release_support import ROOT, ReleaseError, version


def harden(path: Path):
    original = path.read_text()
    marker = "# Fluxcope's only cargo-dist installer extension"
    if marker in original or not original.startswith('#!/bin/sh\n'):
        raise ReleaseError('Unexpected generated installer format or duplicate extension.')
    guard = (ROOT / 'scripts/installer_guard.sh').read_text()
    path.write_text(original.replace('#!/bin/sh\n', '#!/bin/sh\n' + guard + '\n', 1))


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *_):
        pass


def check(directory: Path, value: str):
    arch = {'arm64': 'aarch64', 'aarch64': 'aarch64', 'x86_64': 'x86_64'}[platform.machine()]
    target = arch + ('-apple-darwin' if platform.system() == 'Darwin' else '-unknown-linux-gnu')
    archive = directory / f'fluxcope-{target}.tar.xz'
    installer = directory / 'fluxcope-installer.sh'
    with tempfile.TemporaryDirectory(prefix='fluxcope-installer-') as temporary:
        root = Path(temporary)
        serve = root / 'downloads'
        serve.mkdir()
        shutil.copy2(archive, serve / archive.name)
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), partial(QuietHandler, directory=str(serve)))
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            for damaged in (False, True):
                case = root / ('damaged' if damaged else 'valid')
                case.mkdir()
                if damaged:
                    payload = bytearray(archive.read_bytes())
                    payload[len(payload) // 2] ^= 1
                    (serve / archive.name).write_bytes(payload)
                environment = {'PATH': os.environ['PATH'], 'HOME': str(case),
                               'FLUXCOPE_UNMANAGED_INSTALL': str(case / 'bin'),
                               'FLUXCOPE_NO_MODIFY_PATH': '1',
                               'FLUXCOPE_DOWNLOAD_URL': f'http://127.0.0.1:{server.server_port}'}
                result = subprocess.run(['sh', str(installer)], cwd=case, env=environment,
                                        text=True, capture_output=True, timeout=120)
                binary = case / 'bin/fluxcope'
                if damaged:
                    if result.returncode == 0 or binary.exists() or 'checksum mismatch' not in result.stderr + result.stdout:
                        raise ReleaseError('A damaged archive was not rejected by the native installer checksum check.')
                else:
                    if result.returncode or not binary.is_file():
                        raise ReleaseError(f'Valid installer failed: {result.stdout}\n{result.stderr}')
                    installed = subprocess.run([str(binary), '--version'], env=environment, capture_output=True, text=True, check=True)
                    if installed.stdout.strip() != f'fluxcope {value}':
                        raise ReleaseError('Installer selected the wrong version.')
        finally:
            server.shutdown()
            server.server_close()
            thread.join()
    return {'schema': 1, 'version': value, 'target': target, 'valid_install': True, 'damaged_archive_rejected': True}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path, default=ROOT / 'target/release-assets')
    parser.add_argument('--version', required=True)
    parser.add_argument('--harden', action='store_true')
    args = parser.parse_args()
    if args.harden:
        harden(args.directory / 'fluxcope-installer.sh')
    result = check(args.directory.resolve(), version(args.version))
    (args.directory / 'installer-check.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))
