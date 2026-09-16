"""Read-only preview quality, native packaging, and fresh executable verification."""
from __future__ import annotations

import argparse
from datetime import date
import json
import os
from pathlib import Path
import shutil
import sys
import tomllib

from dist_artifacts import digest, verify_archive
from package_check import inspect
from preview_identity import authorize, snapshot, write_identity
from release_support import CONFIG, ROOT, ReleaseError, output, run


def require_cache_mode():
    if os.environ.get('GITHUB_ACTIONS') == 'true' and os.environ.get('ACTIONS_CACHE_MODE') not in ('read', 'none'):
        raise ReleaseError('GitHub did not enforce read-only preview cache permissions.')


def clean_source(source: Path, identity: dict):
    if (run('git', 'rev-parse', 'HEAD', cwd=source) != identity['snapshot']
            or run('git', 'status', '--porcelain', cwd=source)):
        raise ReleaseError('Preview build requires the exact clean metadata snapshot.')


def quality(source: Path, identity: dict, directory: Path):
    require_cache_mode()
    clean_source(source, identity)
    run('cargo', 'fmt', '--all', '--', '--check', cwd=source, capture=False)
    run('cargo', 'clippy', '--all-targets', '--all-features', '--locked', '--', '-D', 'warnings', cwd=source, capture=False)
    run('cargo-deny', '--manifest-path', str(source / 'Cargo.toml'), '--locked', 'check',
        '--config', str(ROOT / 'deny.toml'), 'advisories', 'licenses', 'sources', cwd=source, capture=False)
    exceptions = json.loads((ROOT / '.github/advisory-exceptions.json').read_text())
    if any(date.today() > date.fromisoformat(item['expires']) for item in exceptions.values()):
        raise ReleaseError('Dependency advisory exception expired.')
    # Ignore source-provided gitleaks configuration; use its built-in rules.
    run('gitleaks', 'git', str(source), '--log-opts=HEAD', '--redact', '--no-banner',
        '--config', str(ROOT / 'scripts/preview-gitleaks.toml'), cwd=ROOT, capture=False)
    run('cargo', 'package', '--locked', cwd=source, capture=False)
    crate = source / 'target/package' / f"fluxcope-{identity['version']}.crate"
    package = inspect(crate, 'fluxcope', identity['version'])
    clean_source(source, identity)
    if package['source'] != identity['snapshot'] or package['dirty']:
        raise ReleaseError('Preview package does not match its clean snapshot.')
    directory.mkdir(parents=True, exist_ok=True)
    (directory / 'quality.json').write_text(json.dumps({**identity, 'result': 'passed', 'package': package}) + '\n')


def build(source: Path, identity: dict, target: str, directory: Path):
    require_cache_mode()
    clean_source(source, identity)
    if target not in identity['targets']:
        raise ReleaseError('Target is outside the approved preview profile.')
    run('cargo', 'test', '--all-targets', '--all-features', '--locked', cwd=source, capture=False)
    manifest = json.loads(run('dist', 'build', '--artifacts', 'local', '--target', target,
                              '--tag', identity['tag'], '--output-format', 'json', cwd=source))
    archive_name = f'fluxcope-{target}.tar.xz'
    artifacts = {archive_name, archive_name + '.sha256'}
    paths = [Path(p) if Path(p).is_absolute() else source / p for p in manifest['upload_files']]
    archives = [p for p in paths if p.name == archive_name]
    if (len(archives) != 1 or not artifacts <= {p.name for p in paths}
            or not {p.name for p in paths} <= artifacts | {'sha256.sum'}):
        raise ReleaseError('cargo-dist returned unexpected preview assets.')
    directory.mkdir(parents=True, exist_ok=True)
    for path in paths:
        if path.name in artifacts:
            if path.is_symlink() or not path.is_file() or not path.resolve().is_relative_to(source / 'target'):
                raise ReleaseError('cargo-dist asset is not a regular build output.')
            shutil.copyfile(path, directory / path.name)
    verify_archive(directory / archive_name, target, identity['version'], directory / f'{target}-build-smoke.json', source_root=source)
    clean_source(source, identity)
    # Only public metadata from dist is retained, with no local paths or arbitrary URLs.
    sanitized = {'dist_version': manifest.get('dist_version'), 'announcement_tag': identity['tag'],
                 'target': target, 'artifacts': {name: {'sha256': digest(directory / name)} for name in sorted(artifacts)}}
    (directory / f'{target}-dist-manifest.json').write_text(json.dumps(sanitized, sort_keys=True) + '\n')


def verify(source: Path, identity: dict, target: str, directory: Path):
    require_cache_mode()
    clean_source(source, identity)
    expected = {f'fluxcope-{target}.tar.xz', f'fluxcope-{target}.tar.xz.sha256',
                f'{target}-build-smoke.json', f'{target}-dist-manifest.json'}
    if {p.name for p in directory.iterdir()} != expected or target not in identity['targets']:
        raise ReleaseError('Fresh verifier received unexpected platform assets.')
    verify_archive(directory / f'fluxcope-{target}.tar.xz', target, identity['version'],
                   directory / f'{target}-smoke.json', source_root=source)
    clean_source(source, identity)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('gate', 'prepare', 'quality', 'build', 'verify'))
    parser.add_argument('--source', type=Path)
    parser.add_argument('--identity', type=Path, required=True)
    parser.add_argument('--directory', type=Path)
    parser.add_argument('--target')
    args = parser.parse_args()
    if args.command in ('gate', 'prepare'):
        identity, _ = authorize(exact=args.command == 'gate')
        identity = snapshot(identity, checkout=args.source)
        write_identity(identity, args.identity)
    else:
        identity = json.loads(args.identity.read_text())
        if args.command == 'quality':
            quality(args.source.resolve(), identity, args.directory)
        else:
            {'build': build, 'verify': verify}[args.command](args.source.resolve(), identity, args.target, args.directory)


if __name__ == '__main__':
    main()
