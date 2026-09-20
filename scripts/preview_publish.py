"""Trusted preview publication: validate data, tag a snapshot, attest and upload."""
from __future__ import annotations

import argparse
import base64
import json
import os
from pathlib import Path
import shutil
import tempfile

from dist_artifacts import digest
from preview_artifacts import (BUNDLE, MANIFEST, assemble, download_public, release_for,
                               resolve_preview_tag, validate_public_directory, download_evidence, public_assets)
from preview_identity import REPO, ROOT, authorize, snapshot, validate_pr
from preview_record import read_record
from release_support import ReleaseError, api, output, repository_path, run


def publication_identity() -> tuple[dict, dict]:
    identity, current = authorize(opened=False)
    record = read_record(identity)
    if record and record.get('cleanup'):
        raise ReleaseError('Preview expiry has started; this ID cannot be republished.')
    return snapshot(identity), current


def prepare(directory: Path):
    identity, current = publication_identity()
    release = release_for(identity)
    if release and not release['draft']:
        download_public(identity, directory)
        output('existing', 'true')
        return
    validate_pr(api(repository_path(f"pulls/{identity['pr']}")), identity['source'], exact=False)
    assemble(current, identity, directory)
    output('existing', 'false')


def finalize(directory: Path, bundle: Path):
    shutil.copyfile(bundle, directory / BUNDLE)
    lines = [f'{digest(path)}  {path.name}' for path in sorted(directory.iterdir()) if path.name != 'sha256.sum']
    (directory / 'sha256.sum').write_text('\n'.join(lines) + '\n')


def ensure_tag(identity: dict):
    resolved = resolve_preview_tag(identity)
    if resolved is not None:
        if resolved != identity['snapshot']:
            raise ReleaseError('Existing preview tag points to another snapshot; refusing to move it.')
        return
    token = os.environ['GIT_TOKEN']
    authorization = base64.b64encode(f'x-access-token:{token}'.encode()).decode()
    environment = {**os.environ, 'GIT_CONFIG_COUNT': '2',
                   'GIT_CONFIG_KEY_0': 'http.https://github.com/.extraheader',
                   'GIT_CONFIG_VALUE_0': f'AUTHORIZATION: basic {authorization}',
                   'GIT_CONFIG_KEY_1': 'core.hooksPath', 'GIT_CONFIG_VALUE_1': '/dev/null'}
    try:
        run('git', 'push', f'https://github.com/{REPO}.git',
            f"{identity['snapshot']}:refs/tags/{identity['tag']}", env=environment)
    except ReleaseError:
        if resolve_preview_tag(identity) != identity['snapshot']:
            raise
    if resolve_preview_tag(identity) != identity['snapshot']:
        raise ReleaseError('Preview tag publication was not confirmed.')


def notes(identity: dict, installer: bool = False) -> str:
    command = (f"Install (no GitHub login, Python or checkout needed):\n\n```sh\n"
               f"curl --proto '=https' --tlsv1.2 -fsSL https://github.com/{REPO}/releases/download/{identity['tag']}/fluxcope-preview-installer.sh | sh\n"
               f"```\n\nLaunch: `~/.local/bin/fluxcope-preview-{identity['id']}`. Installation does not start the app or change PATH.\n\n"
               f"Change port: `~/.local/bin/fluxcope-preview-{identity['id']} --port 9010`.\n\n"
               f"Remove executable: `~/.local/bin/fluxcope-preview-{identity['id']} --uninstall` (retains settings). "
               "Add `--purge` to also remove settings, with confirmation.\n\n"
               "Installed previews run offline after download expiry; new installation or repair needs an unexpired release.\n\n") if installer else ''
    return (command + f"Temporary preview of PR #{identity['pr']} at `{identity['source']}`. No merge or stable release is implied.\n\n"
            f"Snapshot: `{identity['snapshot']}` · controller: `{identity['controller']}`.\n\n"
            f"Platforms: {', '.join(identity['targets'])}. macOS 15+; Linux glibc 2.28+.\n\n"
            'macOS binaries are not Developer ID signed or notarized. The debugging proxy listens on all IPv4 interfaces.\n\n'
            f"Downloads expire {identity['retention_days']} days after publication. Protected tags/source remain.\n\n"
            f"Run with isolated state: `just preview-run {identity['id']}`. This verifies provenance before execution.\n")


def download_stage(directory: Path, *, qualified: bool = False):
    identity, current = publication_identity()
    download_evidence(current, 'Preview attest', 'preview-stage', public_assets({**identity, 'format': 2}),
                      directory, limit=1024 * 1024 * 1024)
    validate_public_directory(directory, identity)
    if qualified:
        verify_install_reports(current, identity, directory)


def verify_install_reports(current: dict, identity: dict, directory: Path):
    for target in identity['targets']:
        name = f'{target}-install.json'
        with tempfile.TemporaryDirectory(prefix='preview-install-report-') as temporary:
            root = Path(temporary)
            download_evidence(current, f'Preview install ({target})', f'preview-install-{target}', {name}, root)
            report = json.loads((root / name).read_text())
        if report != {'schema': 1, 'id': identity['id'], 'controller': identity['controller'],
                      'target': target, 'manifest_sha256': digest(directory / MANIFEST), 'result': 'passed'}:
            raise ReleaseError('Installer qualification does not cover these exact signed assets.')


def publish(directory: Path):
    identity, current = publication_identity()
    sums = validate_public_directory(directory, identity)
    manifest = json.loads((directory / MANIFEST).read_text())
    if manifest.get('format', 1) == 2:
        verify_install_reports(current, identity, directory)
    if os.environ.get('IMMUTABLE_RELEASES_ENABLED') != 'true':
        raise ReleaseError('Immutable release setup has not been confirmed.')
    existing = release_for(identity, include_drafts=True, token=os.environ['GIT_TOKEN'])
    if existing and not existing['draft']:
        with tempfile.TemporaryDirectory(prefix='fluxcope-preview-public-') as temporary:
            download_public(identity, Path(temporary))
            if any(digest(Path(temporary) / name) != value for name, value in sums.items()):
                raise ReleaseError('Published preview bytes conflict; nothing was overwritten.')
        return
    validate_pr(api(repository_path(f"pulls/{identity['pr']}")), identity['source'], exact=False)
    ensure_tag(identity)
    if existing is None:
        existing = api(repository_path('releases'), method='POST', payload={
            'tag_name': identity['tag'], 'target_commitish': identity['snapshot'],
            'name': f"Preview #{identity['pr']} · {identity['source'][:12]} · {identity['profile']}",
            'draft': True, 'prerelease': True, 'make_latest': 'false', 'body': notes(identity, manifest.get('format', 1) == 2)})
    if not existing['prerelease'] or existing['tag_name'] != identity['tag']:
        raise ReleaseError('Conflicting preview draft.')
    present = {a['name']: a for a in existing['assets']}
    if len(present) != len(existing['assets']) or not set(present) <= set(sums):
        raise ReleaseError('Draft contains unexpected assets.')
    if any(asset.get('digest') != 'sha256:' + sums[name] for name, asset in present.items()):
        if not api(repository_path(f"releases/{existing['id']}"), token=os.environ['GIT_TOKEN'])['draft']:
            raise ReleaseError('Release became public; refusing replacement.')
        for asset in present.values():
            api(repository_path(f"releases/assets/{asset['id']}"), method='DELETE')
        present = {}
    environment = {**os.environ, 'GH_TOKEN': os.environ['GIT_TOKEN']}
    for name in sorted(sums):
        if name not in present:
            run('gh', 'release', 'upload', identity['tag'], str(directory / name), '--repo', REPO, env=environment)
    reread = api(repository_path(f"releases/{existing['id']}"), token=os.environ['GIT_TOKEN'])
    if ({a['name']: a.get('digest') for a in reread['assets']} != {n: 'sha256:' + s for n, s in sums.items()}
            or resolve_preview_tag(identity) != identity['snapshot']):
        raise ReleaseError('Draft bytes or tag do not match verified publication evidence.')
    validate_pr(api(repository_path(f"pulls/{identity['pr']}")), identity['source'], exact=False)
    api(repository_path(f"releases/{existing['id']}"), method='PATCH',
        payload={'draft': False, 'prerelease': True, 'make_latest': 'false', 'body': notes(identity, manifest.get('format', 1) == 2)})
    with tempfile.TemporaryDirectory(prefix='fluxcope-preview-public-') as temporary:
        release, _ = download_public(identity, Path(temporary))
    print(release['html_url'])


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('prepare', 'finalize', 'publish', 'download-stage', 'prepare-publication'))
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--bundle', type=Path)
    args = parser.parse_args()
    if args.command == 'finalize':
        finalize(args.directory, args.bundle)
    else:
        {'prepare': prepare, 'publish': publish, 'download-stage': download_stage, 'prepare-publication': lambda path: download_stage(path, qualified=True)}[args.command](args.directory)
