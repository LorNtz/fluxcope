#!/usr/bin/env python3
"""Install one pinned official release asset into an explicit disposable directory."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('tool', choices=('cargo-dist', 'release-plz', 'cargo-deny', 'gitleaks'))
    parser.add_argument('--directory', required=True, type=Path)
    args = parser.parse_args()
    machine = {'arm64': 'aarch64', 'x86_64': 'x86_64', 'aarch64': 'aarch64'}[platform.machine()]
    target = machine + ('-apple-darwin' if platform.system() == 'Darwin' else '-unknown-linux-musl')
    pin = json.loads((ROOT / '.github/tool-pins.json').read_text())[args.tool]
    asset = pin['platforms'][target]
    assert asset['url'].startswith('https://github.com/'), 'tool download host must be GitHub'
    with tempfile.TemporaryDirectory(prefix='fluxcope-tool-') as temporary:
        download = Path(temporary) / 'download'
        subprocess.run(['curl', '--fail', '--silent', '--show-error', '--location', '--proto', '=https',
                        '--tlsv1.2', '--retry', '3', '--output', str(download), asset['url']], check=True)
        data = download.read_bytes()
        if hashlib.sha256(data).hexdigest() != asset['sha256']:
            raise RuntimeError('Tool download SHA-256 does not match the reviewed pin')
        with tarfile.open(fileobj=io.BytesIO(data)) as archive:
            members = [m for m in archive.getmembers() if m.isfile() and Path(m.name).name == pin['executable']]
            if len(members) != 1:
                raise RuntimeError('Tool archive must contain exactly one expected executable')
            args.directory.mkdir(parents=True, exist_ok=True)
            destination = args.directory.resolve() / pin['executable']
            destination.write_bytes(archive.extractfile(members[0]).read())
            destination.chmod(0o755)
        result = subprocess.run([str(destination), *pin.get('version_args', ['--version'])], capture_output=True, text=True, check=True)
        if pin['version'] not in result.stdout:
            raise RuntimeError('Installed tool reports the wrong version')
        print(result.stdout.strip())
        if os.environ.get('GITHUB_PATH'):
            with open(os.environ['GITHUB_PATH'], 'a') as output:
                output.write(str(args.directory.resolve()) + '\n')


if __name__ == '__main__':
    main()
