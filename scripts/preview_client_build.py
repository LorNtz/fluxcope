#!/usr/bin/env python3
"""Package only controller-owned code into a native self-contained preview client."""
import argparse
import importlib.metadata
import json
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
        binary_contract(bundle / 'preview-client', target)
        if target.endswith('apple-darwin'):
            binary_contract(bundle / 'gh', target)
        else:
            # The pinned Go verifier can be fully static and require no glibc.
            dynamic = run('readelf', '-l', str(bundle / 'gh'))
            if 'INTERP' in dynamic:
                binary_contract(bundle / 'gh', target)
        notices = [run(str(bundle / 'gh'), 'licenses')]
        license._Printer__setup()
        notices.append('Python\n' + '\n'.join(license._Printer__lines))
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
    parser.add_argument('--target', required=True)
    parser.add_argument('--directory', type=Path, required=True)
    args = parser.parse_args()
    build(args.target, args.directory)
