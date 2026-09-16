#!/usr/bin/env python3
"""Build with cargo-dist and verify the resulting archives; never hand-package binaries."""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib

from release_support import CONFIG, ROOT, ReleaseError, run, version
from installer_check import check as check_installer, harden

DIST = ROOT / 'target/distrib'


def digest(path: Path) -> str:
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def binary_contract(binary: Path, target: str):
    if target.endswith('apple-darwin'):
        commands = run('otool', '-l', str(binary))
        # LC_BUILD_VERSION's minos is unambiguous; SDK and library versions are not OS minima.
        minimum = re.findall(r'\bminos (\d+)\.(\d+)(?:\.(\d+))?', commands)
        if not minimum or any(tuple(int(p or 0) for p in parts) > (15, 0, 0) for parts in minimum):
            raise ReleaseError('Mach-O requires an OS newer than the declared macOS 15 minimum.')
    else:
        symbols = run('readelf', '--version-info', str(binary))
        versions = [tuple(map(int, v.split('.'))) for v in re.findall(r'GLIBC_([0-9.]+)', symbols)]
        if not versions or max(versions) > (2, 28):
            raise ReleaseError('ELF requires glibc newer than 2.28, or has no verifiable glibc requirement.')
        dependencies = run('ldd', str(binary))
        if 'not found' in dependencies:
            raise ReleaseError('ELF has unresolved runtime dependencies.')


def extract_binary(archive: Path, binary: Path):
    """Extract only the executable from a bounded, regular-file native archive."""
    with tarfile.open(archive) as compressed:
        members = []
        total = 0
        for member in compressed:
            total += member.size
            if (len(members) >= 1000 or total > 512 * 1024 * 1024
                    or not (member.isfile() or member.isdir())
                    or Path(member.name).is_absolute() or '..' in Path(member.name).parts):
                raise ReleaseError('Unexpected archive type, path or expanded size.')
            members.append(member)
        if len({m.name for m in members}) != len(members):
            raise ReleaseError('Duplicate archive member.')
        binaries = [m for m in members if m.isfile() and Path(m.name).name == 'fluxcope']
        if len(binaries) != 1 or not binaries[0].mode & 0o111:
            raise ReleaseError('Expected one executable Fluxcope binary in archive.')
        with compressed.extractfile(binaries[0]) as source, binary.open('wb') as destination:
            shutil.copyfileobj(source, destination, length=1024 * 1024)
        binary.chmod(0o755)


def verify_archive(archive: Path, target: str, value: str, report: Path, *, source_root: Path = ROOT):
    checksum = archive.with_name(archive.name + '.sha256').read_text().split()[0]
    if digest(archive) != checksum:
        raise ReleaseError('Archive does not match its native cargo-dist checksum.')
    with tempfile.TemporaryDirectory(prefix='fluxcope-archive-') as directory:
        binary = Path(directory) / 'fluxcope'
        extract_binary(archive, binary)
        binary_contract(binary, target)
        args = [sys.executable, str(ROOT / 'scripts/smoke.py'), '--binary', str(binary),
                '--version', value, '--target', target, '--report', str(report)]
        if target.endswith('apple-darwin'):
            args += ['--require-macos', '15']
        subprocess.run(args, check=True)
    result = json.loads(report.read_text())
    result.update(archive=archive.name, archive_sha256=digest(archive), source=run('git', 'rev-parse', 'HEAD', cwd=source_root),
                  dirty=bool(run('git', 'status', '--porcelain', cwd=source_root)))
    report.write_text(json.dumps(result, indent=2) + '\n')


def local(target: str, value: str):
    DIST.mkdir(parents=True, exist_ok=True)
    manifest = run('dist', 'build', '--artifacts', 'local', '--target', target, '--tag', f'v{value}', '--output-format', 'json')
    parsed = json.loads(manifest)
    archives = [Path(p) for p in parsed['upload_files'] if p.endswith('.tar.xz')]
    if len(archives) != 1:
        raise ReleaseError('cargo-dist must produce exactly one archive for this target.')
    (DIST / f'{target}-dist-manifest.json').write_text(manifest + '\n')
    verify_archive(archives[0], target, value, DIST / f'{target}-smoke.json')
    output = ROOT / 'target/local-artifacts'
    output.mkdir(parents=True, exist_ok=True)
    for path in [*map(Path, parsed['upload_files']), DIST / f'{target}-dist-manifest.json', DIST / f'{target}-smoke.json']:
        shutil.copy2(path, output / path.name)


def global_artifacts(value: str):
    expected = {p['target'] for p in CONFIG['platforms']}
    reports = [json.loads((DIST / f'{target}-smoke.json').read_text()) for target in sorted(expected)]
    source = run('git', 'rev-parse', 'HEAD')
    for report in reports:
        if report['result'] != 'passed' or report['version'] != value or report['source'] != source or report.get('dirty', True):
            raise ReleaseError('Platform verification has the wrong version or source.')
        if report['archive_sha256'] != digest(DIST / report['archive']):
            raise ReleaseError('Platform evidence does not match the downloaded archive.')
    manifest = run('dist', 'build', '--artifacts', 'global', '--tag', f'v{value}', '--output-format', 'json')
    (DIST / 'global-dist-manifest.json').write_text(manifest + '\n')
    public = ROOT / 'target/release-assets'
    public.mkdir(parents=True, exist_ok=True)
    paths = [DIST / report['archive'] for report in reports]
    paths += [DIST / (report['archive'] + '.sha256') for report in reports]
    paths += [DIST / 'fluxcope-installer.sh', DIST / 'fluxcope.rb']
    paths += [DIST / f'{target}-smoke.json' for target in sorted(expected)]
    # Preserve dist's manifest and checksum layout; add the global installer and
    # evidence to its checksum coverage before immutable publication.
    clean_manifest = run('dist', 'manifest', '--artifacts', 'all', '--tag', f'v{value}', '--no-local-paths', '--output-format', 'json')
    (public / 'dist-manifest.json').write_text(clean_manifest + '\n')
    for path in paths:
        shutil.copy2(path, public / path.name)
    native = (DIST / 'sha256.sum').read_text()
    checksum_names = set()
    for line in native.splitlines():
        if not line.strip():
            continue
        checksum, filename = line.split(maxsplit=1)
        filename = filename.lstrip('*')
        if Path(filename).name != filename:
            raise ReleaseError('Native checksum contains a nonlocal filename.')
        path = DIST / filename
        if not path.is_file() or digest(path) != checksum:
            raise ReleaseError('Native global checksum does not match its artifact.')
        if (public / filename).exists():
            checksum_names.add(filename)
    if not {report['archive'] for report in reports} <= checksum_names:
        raise ReleaseError('Native global checksum is missing a platform archive.')
    harden(public / 'fluxcope-installer.sh')
    installer_result = check_installer(public, value)
    (public / 'installer-check.json').write_text(json.dumps(installer_result, indent=2) + '\n')
    if os.environ.get('GITHUB_EVENT_NAME') == 'workflow_dispatch':
        from release_gate import merged_release
        from release_evidence import source_package
        identity = merged_release(source, expected=value)
        package, _ = source_package(identity)
        evidence = {**identity, 'schema': 1, 'package': package}
    else:
        evidence = {'schema': 1, 'source': source, 'version': value, 'channel': 'preview'}
    (public / 'source-evidence.json').write_text(json.dumps(evidence, indent=2) + '\n')
    lines = [f'{digest(path)}  {path.name}' for path in sorted(public.iterdir()) if path.name != 'sha256.sum']
    (public / 'sha256.sum').write_text('\n'.join(lines) + '\n')
    print(f'Verified {len(reports)} native archives; assembled global installers, manifest and checksums.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('local', 'global'))
    parser.add_argument('--target')
    parser.add_argument('--version', required=True)
    args = parser.parse_args()
    value = version(args.version)
    package = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']
    if package['version'] != value:
        raise ReleaseError('Build version differs from Cargo.toml.')
    if args.mode == 'local':
        if args.target not in {p['target'] for p in CONFIG['platforms']}:
            raise ReleaseError('Unsupported target.')
        local(args.target, value)
    else:
        global_artifacts(value)


if __name__ == '__main__':
    main()
