#!/usr/bin/env python3
"""Verify the Cargo publication allowlist and optionally install the exact unpacked crate."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import subprocess
import tempfile
import tarfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def inspect(crate: Path, name: str, version: str) -> dict:
    prefix = f'{name}-{version}/'
    allowed = {'Cargo.toml', 'Cargo.toml.orig', 'Cargo.lock', '.cargo_vcs_info.json',
               'README.md', 'CHANGELOG.md', 'LICENSE-MIT', 'LICENSE-APACHE'}
    with tarfile.open(crate) as archive:
        for member in archive.getmembers():
            if not member.name.startswith(prefix) or not member.isfile():
                raise ValueError(f'Unexpected crate member: {member.name}')
            path = PurePosixPath(member.name.removeprefix(prefix))
            if '..' in path.parts or not (str(path) in allowed or (path.parts[0] == 'src' and path.suffix == '.rs')):
                raise ValueError(f'Unexpected published path: {path}')
        manifest = tomllib.loads(archive.extractfile(prefix + 'Cargo.toml').read().decode())
        assert manifest['package']['name'] == name and manifest['package']['version'] == version
        assert manifest['package']['license'] == 'MIT OR Apache-2.0'
        assert all((ROOT / filename).is_file() for filename in ('LICENSE-MIT', 'LICENSE-APACHE'))
        vcs = json.load(archive.extractfile(prefix + '.cargo_vcs_info.json'))
    return {'schema': 1, 'crate': name, 'version': version,
            'source': vcs['git']['sha1'], 'dirty': vcs['git'].get('dirty', False),
            'sha256': hashlib.sha256(crate.read_bytes()).hexdigest(), 'filename': crate.name}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--install', action='store_true')
    parser.add_argument('--allow-dirty', action='store_true', help='Local investigation only; never publish this package')
    parser.add_argument('--report', type=Path, default=ROOT / 'target/package-report.json')
    args = parser.parse_args()
    package = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']
    subprocess.run(['cargo', 'package', '--locked', *(['--allow-dirty'] if args.allow_dirty else [])], cwd=ROOT, check=True)
    crate = ROOT / 'target/package' / f"{package['name']}-{package['version']}.crate"
    report = inspect(crate, package['name'], package['version'])
    if not args.allow_dirty and report['dirty']:
        raise ValueError('Publication requires a clean source checkout')
    if args.install:
        with tempfile.TemporaryDirectory(prefix='fluxcope-package-') as directory:
            root = Path(directory)
            with tarfile.open(crate) as archive:
                archive.extractall(root, filter='data')
            subprocess.run(['cargo', 'install', '--path', str(root / f"{package['name']}-{package['version']}"),
                            '--locked', '--target-dir', str(ROOT / 'target/package-install'),
                            '--root', str(root / 'installed')], check=True)
            result = subprocess.run([str(root / 'installed/bin/fluxcope'), '--version'], capture_output=True, text=True, check=True)
            assert result.stdout.strip() == f"fluxcope {package['version']}"
            result = subprocess.run([str(root / 'installed/bin/fluxcope'), '--help'], capture_output=True, text=True, check=True)
            assert 'Usage:' in result.stdout and '--version' in result.stdout
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
