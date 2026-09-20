#!/usr/bin/env python3
"""Package only controller-owned code into a native self-contained preview client."""
import argparse
import importlib.metadata
import json
import re
from pathlib import Path
import sys
import tarfile
import tempfile
import zipfile

from dist_artifacts import binary_contract
from preview_client.model import client_asset, digest, PreviewError
from preview_client.remote import clean_environment
from preview_client.state import host_target
from release_support import ROOT, run


def prepare_python(target, directory):
    """Fetch a checksum-pinned native interpreter with the shared library PyInstaller needs."""
    if target != host_target():
        raise PreviewError('Packaging Python must match the native platform.')
    pins = json.loads((ROOT / '.github/preview-client-pins.json').read_text())
    pin = pins['python'][target]
    if directory.exists():
        raise PreviewError('Use a fresh packaging runtime directory.')
    with tempfile.TemporaryDirectory(prefix='preview-python-') as temporary:
        archive = Path(temporary) / 'python.tar.gz'
        url = f"https://github.com/astral-sh/python-build-standalone/releases/download/{pins['python_release']}/{pin['asset']}"
        run('curl', '--fail', '--location', '--retry', '3', '--max-time', '180',
            '--max-filesize', str(128 * 1024 * 1024), '--output', str(archive), url)
        if digest(archive) != pin['sha256']:
            raise PreviewError('Pinned packaging Python checksum mismatch.')
        directory.mkdir()
        with tarfile.open(archive) as stream:
            stream.extractall(directory, filter='data')
    run(str(directory / 'python/bin/python3'), '-c',
        'import sysconfig; assert sysconfig.get_config_var("Py_ENABLE_SHARED") == 1')


def client_contract(binary, target):
    if target.endswith('apple-darwin'):
        commands = run('otool', '-l', str(binary))
        versions = re.findall(r'\bminos (\d+)\.(\d+)(?:\.(\d+))?', commands)
        versions += re.findall(r'cmd LC_VERSION_MIN_MACOSX\s+cmdsize \d+\s+version (\d+)\.(\d+)(?:\.(\d+))?', commands)
        if not versions or any(tuple(int(p or 0) for p in value) > (15, 0, 0) for value in versions):
            raise PreviewError('Private runtime requires an OS newer than macOS 15, or lacks a verifiable minimum.')
    elif 'INTERP' in run('readelf', '-l', str(binary)):
        binary_contract(binary, target)
    # A static ELF has no glibc dependency. readelf and native execution still validate it.


def build(target, output):
    if target != host_target():
        raise PreviewError('Private clients must be built on their native platform.')
    pins = json.loads((ROOT / '.github/preview-client-pins.json').read_text())
    pin = pins['gh'][target]
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='preview-client-build-') as temporary:
        work = Path(temporary)
        bundle = work / 'bundle'
        bundle.mkdir()
        run(sys.executable, '-m', 'PyInstaller', '--noconfirm', '--clean', '--onefile',
            '--name', 'preview-client', '--paths', str(ROOT / 'scripts'),
            '--distpath', str(bundle), '--workpath', str(work / 'work'), '--specpath', str(work),
            str(ROOT / 'scripts/preview_client_entry.py'), capture=False)
        archive = work / pin['asset']
        url = f"https://github.com/cli/cli/releases/download/v{pins['gh_version']}/{pin['asset']}"
        run('curl', '--fail', '--location', '--retry', '3', '--max-time', '180', '--output', str(archive), url)
        if digest(archive) != pin['sha256']:
            raise PreviewError('Pinned GitHub verifier checksum mismatch.')
        if archive.suffix == '.zip':
            with zipfile.ZipFile(archive) as source:
                matches = [n for n in source.namelist() if n.endswith('/bin/gh')]
                if len(matches) != 1:
                    raise PreviewError('Expected one pinned verifier executable.')
                (bundle / 'gh').write_bytes(source.read(matches[0]))
        else:
            with tarfile.open(archive) as source:
                matches = [m for m in source if m.isfile() and m.name.endswith('/bin/gh')]
                if len(matches) != 1:
                    raise PreviewError('Expected one pinned verifier executable.')
                with source.extractfile(matches[0]) as stream:
                    (bundle / 'gh').write_bytes(stream.read())
        (bundle / 'gh').chmod(0o755)
        # Capture Sigstore roots with the pinned verifier; installed verification is offline.
        roots = run(str(bundle / 'gh'), 'attestation', 'trusted-root', env=clean_environment(work))
        (bundle / 'trusted-root.jsonl').write_text(roots + '\n')
        run(str(bundle / 'preview-client'), '--help')
        for name in ('preview-client', 'gh'):
            client_contract(bundle / name, target)
        notices = [run(str(bundle / 'gh'), 'licenses')]
        license._Printer__setup()
        notices.append('Python\n' + '\n'.join(license._Printer__lines))
        for path in sorted((Path(sys.base_prefix) / 'licenses').glob('*')):
            if path.is_file():
                notices.append(path.name + '\n' + path.read_text())
        for name in ('pyinstaller', 'ruamel.yaml', 'certifi'):
            distribution = importlib.metadata.distribution(name)
            for path in distribution.files:
                if Path(path).name.upper().startswith(('LICENSE', 'COPYING')):
                    notices.append(name + '\n' + distribution.locate_file(path).read_text())
        (bundle / 'licenses.txt').write_text('\n\n'.join(notices))
        with tarfile.open(output / client_asset(target), 'w:xz') as archive:
            for path in sorted(bundle.iterdir()):
                archive.add(path, arcname=path.name, recursive=False)
        print(output / client_asset(target))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('bundle', 'python'), nargs='?', default='bundle')
    parser.add_argument('--target', required=True)
    parser.add_argument('--directory', type=Path, required=True)
    args = parser.parse_args()
    {'bundle': build, 'python': prepare_python}[args.command](args.target, args.directory)
