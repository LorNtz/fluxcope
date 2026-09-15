#!/usr/bin/env python3
"""Validate all release bytes before creating a draft; never replace remote assets."""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
from urllib.request import urlopen

from dist_artifacts import digest
from release_evidence import source_package
from release_gate import dispatch_gate, optional_api, resolve_tag
from release_source import check_registry
from release_support import CONFIG, REPO, ROOT, ReleaseError, api, repository_path, run

SIGNER = f'{REPO}/.github/workflows/release.yml'


def checksums(data: str) -> dict[str, str]:
    result = {}
    for line in data.splitlines():
        match = re.fullmatch(r'([0-9a-f]{64})  ([a-zA-Z0-9_.-]+)', line)
        if not match or match[2] in result:
            raise ReleaseError('Malformed or duplicate release checksum entry.')
        result[match[2]] = match[1]
    return result


def expected_assets() -> set[str]:
    return ({f"fluxcope-{p['target']}.tar.xz" for p in CONFIG['platforms']}
            | {f"fluxcope-{p['target']}.tar.xz.sha256" for p in CONFIG['platforms']}
            | {f"{p['target']}-smoke.json" for p in CONFIG['platforms']}
            | {'fluxcope-installer.sh', 'fluxcope.rb', 'dist-manifest.json', 'installer-check.json', 'source-evidence.json', 'sha256.sum'})


def verify_provenance(path: Path, identity: dict):
    run('gh', 'attestation', 'verify', str(path), '--repo', REPO,
        '--signer-workflow', SIGNER, '--source-digest', identity['source'],
        '--source-ref', f"refs/tags/v{identity['version']}", '--deny-self-hosted-runners', capture=False)


def validate_local(directory: Path, identity: dict, *, provenance: bool = True) -> dict:
    paths = list(directory.iterdir())
    if {p.name for p in paths} != expected_assets() or any(not p.is_file() or p.is_symlink() for p in paths):
        raise ReleaseError('Release assets differ from the explicit artifact allowlist.')
    sums = checksums((directory / 'sha256.sum').read_text())
    if set(sums) != expected_assets() - {'sha256.sum'}:
        raise ReleaseError('Checksums do not cover every release asset.')
    if any(digest(directory / name) != checksum for name, checksum in sums.items()):
        raise ReleaseError('A release asset differs from its verified checksum.')
    manifest = json.loads((directory / 'dist-manifest.json').read_text())
    if not set(manifest['artifacts']) <= expected_assets():
        raise ReleaseError('The native manifest references an unpublished artifact.')
    evidence = json.loads((directory / 'source-evidence.json').read_text())
    package = evidence.get('package', {})
    if (evidence.get('schema') != 1 or evidence.get('source') != identity['source']
            or evidence.get('version') != identity['version'] or evidence.get('channel') != identity['channel']
            or package.get('schema') != 1 or package.get('dirty', True)
            or package.get('source') != identity['source'] or package.get('version') != identity['version']):
        raise ReleaseError('Public source evidence does not match this release.')
    if identity['channel'] == 'stable':
        check_registry(identity, package['sha256'])
    for platform in CONFIG['platforms']:
        target = platform['target']
        report = json.loads((directory / f'{target}-smoke.json').read_text())
        archive = f'fluxcope-{target}.tar.xz'
        if (directory / (archive + '.sha256')).read_text().split()[0] != sums[archive]:
            raise ReleaseError('Native archive sidecar checksum does not match the published bytes.')
        if (report.get('schema') != 1 or report.get('dirty', True) or report.get('result') != 'passed'
                or report.get('source') != identity['source'] or report.get('version') != identity['version']
                or report.get('target') != target or report.get('archive') != archive
                or report.get('archive_sha256') != sums[archive]):
            raise ReleaseError(f'Invalid application evidence for {target}.')
        if provenance:
            verify_provenance(directory / archive, identity)
    installer = json.loads((directory / 'installer-check.json').read_text())
    if (installer.get('schema') != 1 or installer.get('version') != identity['version']
            or installer.get('valid_install') is not True or installer.get('damaged_archive_rejected') is not True):
        raise ReleaseError('Installer integrity fixture did not pass.')
    if provenance:
        verify_provenance(directory / 'fluxcope-installer.sh', identity)
    return {**sums, 'sha256.sum': digest(directory / 'sha256.sum')}


def download_public(identity: dict, directory: Path) -> dict:
    release = optional_api(f"releases/tags/v{identity['version']}")
    if not release or release['draft'] or not release.get('immutable'):
        raise ReleaseError('The expected immutable GitHub Release is not publicly published.')
    if release['prerelease'] != (identity['channel'] == 'rc') or resolve_tag(identity['version']) != identity['source']:
        raise ReleaseError('Release channel or tag identity changed.')
    assets = release['assets']
    if {a['name'] for a in assets} != expected_assets() or len(assets) != len(expected_assets()):
        raise ReleaseError('Published assets differ from the release allowlist.')
    directory.mkdir(parents=True, exist_ok=True)
    for asset in assets:
        if asset['size'] > 128 * 1024 * 1024:
            raise ReleaseError('Unexpectedly large release asset.')
        # Deliberately unauthenticated: Homebrew and users must be able to fetch it.
        url = f"https://github.com/{REPO}/releases/download/v{identity['version']}/{asset['name']}"
        destination = directory / asset['name']
        with urlopen(url, timeout=60) as response, destination.open('wb') as stream:
            size = 0
            while chunk := response.read(1024 * 1024):
                size += len(chunk)
                if size > 128 * 1024 * 1024:
                    raise ReleaseError('Public download exceeds its size limit.')
                stream.write(chunk)
        if destination.stat().st_size != asset['size'] or asset.get('digest') != 'sha256:' + digest(destination):
            raise ReleaseError('Public download differs from the GitHub asset digest.')
    validate_local(directory, identity)
    report = {**identity, 'schema': 1, 'result': 'passed', 'signer': SIGNER,
              'assets': {a['name']: a['digest'] for a in assets}}
    (ROOT / 'target').mkdir(exist_ok=True)
    (ROOT / 'target/public-verification.json').write_text(json.dumps(report, indent=2) + '\n')
    return release


def notes(identity: dict) -> str:
    changelog = (ROOT / 'CHANGELOG.md').read_text()
    heading = re.search(rf'^##\s+\[?{re.escape(identity["version"])}\]?(?=[\s(]|$)', changelog, re.MULTILINE)
    if not heading:
        raise ReleaseError('Committed changelog has no entry for this release.')
    rest = changelog[heading.end():]
    end = re.search(r'^##\s', rest, re.MULTILINE)
    entry = changelog[heading.start():heading.end()] + (rest[:end.start()] if end else rest)
    return (entry.strip() + f'\n\nSource: `{identity["source"]}` · reviewed PR #{identity["pr"]}.\n\n'
            'macOS 15+ (Apple Silicon / Intel); Linux glibc 2.28+ (ARM64 / x86_64).\n\n'
            'Fluxcope is a local debugging MITM proxy. It listens on all IPv4 interfaces; use a trusted network. '
            'Protect the generated CA private key and remove any installed CA trust when finished. '
            'macOS binaries are not Developer ID signed or notarized.\n\n'
            f'Installation and verification: https://github.com/{REPO}/blob/v{identity["version"]}/README.md\n\n'
            f'Provenance signer: `{SIGNER}`; verify the exact tag and source digest.\n')


def publish(directory: Path, identity: dict):
    sums = validate_local(directory, identity)
    # Setup records this only after enabling immutable releases through GitHub's
    # admin API. The published API response is checked independently below.
    if os.environ.get('IMMUTABLE_RELEASES_ENABLED') != 'true':
        raise ReleaseError('Repository immutable release setup has not been confirmed.')
    release = optional_api(f"releases/tags/v{identity['version']}")
    if release and not release['draft']:
        with tempfile.TemporaryDirectory(prefix='fluxcope-public-verify-') as temporary:
            download_public(identity, Path(temporary))
            if any(digest(Path(temporary) / name) != checksum for name, checksum in sums.items()):
                raise ReleaseError('Published immutable assets differ from this run; nothing was overwritten.')
        return
    if release is None:
        release = api(repository_path('releases'), method='POST', payload={
            'tag_name': f"v{identity['version']}", 'target_commitish': identity['source'],
            'name': f"Fluxcope {identity['version']}", 'draft': True,
            'prerelease': identity['channel'] == 'rc', 'make_latest': 'false', 'body': notes(identity)})
    if release['prerelease'] != (identity['channel'] == 'rc'):
        raise ReleaseError('Existing draft has a different release channel.')
    present = {asset['name']: asset for asset in release['assets']}
    if len(present) != len(release['assets']) or not set(present) <= set(sums):
        raise ReleaseError('Existing draft has duplicate or unexpected assets.')
    if any(asset.get('digest') != 'sha256:' + sums[name] for name, asset in present.items()):
        # All replacement bytes and attestations were verified above. A draft is
        # private staging state; reset the entire set instead of mixing builds.
        if not api(repository_path(f"releases/{release['id']}"))['draft']:
            raise ReleaseError('Draft became public; no asset will be replaced.')
        for asset in present.values():
            api(repository_path(f"releases/assets/{asset['id']}"), method='DELETE')
        present = {}
    for name, checksum in sums.items():
        if name in present:
            if present[name].get('digest') != 'sha256:' + checksum:
                raise ReleaseError(f'Existing draft asset {name} conflicts; refusing replacement.')
        else:
            run('gh', 'release', 'upload', f"v{identity['version']}", str(directory / name), '--repo', REPO, capture=False)
    verified = api(repository_path(f"releases/{release['id']}"))
    if {a['name']: a.get('digest') for a in verified['assets']} != {n: 'sha256:' + d for n, d in sums.items()}:
        raise ReleaseError('Draft upload verification failed.')
    if resolve_tag(identity['version']) != identity['source']:
        raise ReleaseError('Tag moved before publication.')
    api(repository_path(f"releases/{release['id']}"), method='PATCH', payload={
        'draft': False, 'make_latest': 'false' if identity['channel'] == 'rc' else 'true', 'body': notes(identity)})
    with tempfile.TemporaryDirectory(prefix='fluxcope-public-verify-') as temporary:
        published = download_public(identity, Path(temporary))
    print(published['html_url'])


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('validate', 'publish', 'verify'))
    parser.add_argument('--tag', required=True)
    parser.add_argument('--directory', type=Path, default=ROOT / 'target/release-assets')
    args = parser.parse_args()
    identity = dispatch_gate(args.tag)
    if identity['channel'] == 'stable':
        package, _ = source_package(identity)
        check_registry(identity, package['sha256'])
    if args.mode == 'publish':
        publish(args.directory, identity)
    elif args.mode == 'verify':
        download_public(identity, args.directory)
    else:
        validate_local(args.directory, identity)
