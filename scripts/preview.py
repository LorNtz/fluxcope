#!/usr/bin/env python3
"""Publish, follow, retry and run isolated previews of committed development work."""
from __future__ import annotations

import argparse
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import platform
import shutil
import socket
import subprocess
import sys
import tempfile
import time

from dist_artifacts import digest, extract_binary
from preview_artifacts import BUNDLE, download_asset, download_public, provenance, release_for
from preview_identity import (BASE, PROFILES, REPO, TITLE, WORKFLOW, identity_from_run,
                              platforms, preview_id, timestamp, trusted_run, validate_pr)
from preview_record import read_record
from release import choose, confirm, feature_pr, pull_requests
from release_support import (ReleaseError, api, dispatch, repository_path, run,
                             safe_text, verify_local_repository)


def runs(pr: int) -> list[dict]:
    matches = []
    for page in range(1, 11):
        response = api(repository_path(f'actions/workflows/{WORKFLOW}/runs?event=workflow_dispatch&branch={BASE}&per_page=100&page={page}'))
        for item in response['workflow_runs']:
            match = TITLE.fullmatch(item['display_title'])
            if match and int(match[1]) == pr:
                matches.append(item)
        if len(response['workflow_runs']) < 100:
            break
    return matches


def current_pr() -> dict:
    branch = run('git', 'symbolic-ref', '--quiet', '--short', 'HEAD')
    matches = [p for p in pull_requests('all') if p['head']['ref'] == branch
               and (p['head'].get('repo') or {}).get('full_name') == REPO]
    return choose(matches, lambda p: f"#{p['number']} {p['title']} ({p['state']})")


def select_run(identifier: str | None, *, runnable: bool = False) -> dict:
    if identifier:
        return trusted_run(preview_id(identifier))
    candidates = runs(current_pr()['number'])
    if runnable:
        native = host_target()
        available = []
        for item in candidates:
            identity = identity_from_run(item)
            if native not in identity['targets'] or item['conclusion'] != 'success':
                continue
            release = release_for(identity)
            if (release and not release['draft'] and release.get('published_at')
                    and datetime.now(timezone.utc) < timestamp(release['published_at']) + timedelta(days=identity['retention_days'])):
                available.append(item)
        candidates = available
    selected = choose(candidates, lambda r: f"{r['id']} {r['display_title']} ({r['status']}/{r['conclusion']})")
    return trusted_run(selected['id'])


def status(identifier: int):
    deadline = time.monotonic() + 6 * 60 * 60
    previous = None
    while time.monotonic() < deadline:
        current = trusted_run(identifier)
        identity = identity_from_run(current)
        record = read_record(identity)
        if record and record.get('cleanup'):
            state = 'expired' if record['cleanup'] == 'complete' else 'cleanup-pending'
            raise ReleaseError(f'Preview {identifier}: {state}; request a new preview.')
        message = (current['status'], current['conclusion'], record.get('state') if record else None)
        if message != previous:
            print(f"Preview {identifier}: {' / '.join(str(p) for p in message if p)}\n{current['html_url']}")
            previous = message
        if current['status'] == 'completed':
            if record and record['state'] == 'complete':
                with tempfile.TemporaryDirectory(prefix='fluxcope-preview-status-') as directory:
                    release, _ = download_public(identity, Path(directory), metadata_only=True)
                print(f"Download: {release['html_url']}\nRun: just preview-run {identifier}")
                return
            raise ReleaseError(f'Preview needs attention. Inspect the workflow, then: just preview-retry {identifier}')
        time.sleep(30)
    raise ReleaseError(f'Local wait ended; remote work continues. Resume: just preview-status {identifier}')


def reusable(current: dict) -> bool:
    if current['status'] != 'completed':
        return True
    identity = identity_from_run(current)
    record = read_record(identity)
    if record and record.get('cleanup'):
        return False
    release = release_for(identity)
    return bool(release and not release['draft'] and release.get('published_at')
                and datetime.now(timezone.utc) < timestamp(release['published_at']) + timedelta(days=identity['retention_days']))


def publish(profile: str, force_new: bool):
    platforms(profile)
    permission = api(repository_path()).get('permissions', {})
    if not (permission.get('maintain') or permission.get('admin')):
        raise ReleaseError('Preview publication requires maintain/admin permission.')
    pr = feature_pr(resume=False, draft=True)
    head = run('git', 'rev-parse', 'HEAD')
    validate_pr(pr, head, exact=True)
    matching = [r for r in runs(pr['number']) if identity_from_run(r)['source'] == head
                and identity_from_run(r)['profile'] == profile]
    candidates = [r for r in matching if reusable(r)] if not force_new else []
    if candidates:
        selected = choose(candidates, lambda r: f"{r['id']} ({r['status']}/{r['conclusion']}) {r['html_url']}")
        status(selected['id'])
        return
    print(f"\nPublic GitHub preview of PR #{pr['number']}\nSource: {head}\nProfile: {profile}\n"
          'Downloads: 30 days. Protected tag/source remain in Git history.\n'
          'Your branch remains unmerged; stable installation and publication channels are unchanged.')
    confirm('Publish this exact committed source as a temporary preview?')
    requested = datetime.now(timezone.utc)
    actor = api('user')['login']
    previous_ids = {r['id'] for r in matching}
    dispatch_error = None
    try:
        dispatch(WORKFLOW, BASE, {'pr': str(pr['number']), 'source_sha': head, 'profile': profile})
    except ReleaseError as error:
        dispatch_error = error
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        found = [r for r in runs(pr['number']) if r['id'] not in previous_ids
                 and identity_from_run(r)['source'] == head and identity_from_run(r)['profile'] == profile
                 and r['actor']['login'] == actor and timestamp(r['created_at']) >= requested - timedelta(seconds=5)]
        if found:
            selected = choose(found, lambda r: f"{r['id']} {r['html_url']}")
            status(selected['id'])
            return
        time.sleep(5)
    detail = f' ({dispatch_error})' if dispatch_error else ''
    raise ReleaseError('Dispatch outcome is not yet confirmed' + detail + '. No duplicate was sent. Run just preview-status before retrying.')


def retry(current: dict):
    identity = identity_from_run(current)
    record = read_record(identity)
    if record and record.get('cleanup'):
        raise ReleaseError('Expired previews cannot be retried; request a new ID.')
    if current['status'] != 'completed':
        status(current['id'])
        return
    if current['conclusion'] == 'success':
        status(current['id'])
        return
    print(f"Retry preview {identity['id']} at {identity['source']} using controller {identity['controller']}.\n"
          'Failed jobs rerun against the original identity. A source/tooling fix requires a new preview.')
    confirm('Rerun the original failed jobs for this preview?')
    api(repository_path(f"actions/runs/{identity['id']}/rerun-failed-jobs"), method='POST')
    # Wait until GitHub acknowledges a new attempt before observing completion.
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        refreshed = trusted_run(identity['id'])
        if refreshed['run_attempt'] > current['run_attempt'] or refreshed['status'] != 'completed':
            status(identity['id'])
            return
        time.sleep(5)
    raise ReleaseError(f'Rerun acknowledgement is delayed. Resume with just preview-status {identity["id"]}.')


def host_target() -> str:
    machine = {'arm64': 'aarch64', 'aarch64': 'aarch64', 'x86_64': 'x86_64'}.get(platform.machine())
    system = platform.system()
    if not machine or system not in ('Darwin', 'Linux'):
        raise ReleaseError('Preview execution supports native macOS/Linux ARM64 and x86-64.')
    if system == 'Darwin' and int(platform.mac_ver()[0].split('.')[0]) < 15:
        raise ReleaseError('macOS previews require macOS 15 or newer.')
    return machine + ('-apple-darwin' if system == 'Darwin' else '-unknown-linux-gnu')


def private_directory(path: Path):
    if path.is_symlink():
        raise ReleaseError(f'Preview state must not be a symlink: {path}')
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    if path.stat().st_uid != os.getuid():
        raise ReleaseError('Preview state is owned by another user.')
    path.chmod(0o700)


def execute(current: dict):
    identity = identity_from_run(current)
    record = read_record(identity)
    if not record or record['state'] != 'complete' or record.get('cleanup'):
        raise ReleaseError('Only a complete, unexpired preview can be run.')
    target = host_target()
    base = Path.home() / '.cache/fluxcope/previews'
    # Check every existing component before creating private descendants.
    for directory in (Path.home() / '.cache', base.parent, base, base / str(identity['id']), base / str(identity['id']) / target):
        if directory.is_symlink():
            raise ReleaseError('Preview cache path traverses a symlink.')
    root = base / str(identity['id']) / target
    for directory in (base.parent, base, root.parent, root, root / 'bin', root / 'home', root / 'home/.fluxcope'):
        private_directory(directory)
    with tempfile.TemporaryDirectory(prefix='download-', dir=root) as temporary:
        release, manifest = download_public(identity, Path(temporary), metadata_only=True)
        if target not in identity['targets']:
            raise ReleaseError('This preview does not include your host; request its native profile or all.')
        # The signed fresh verification report also binds the executable digest.
        report_name = f'{target}-smoke.json'
        asset = next(a for a in release['assets'] if a['name'] == report_name)
        download_asset(identity, asset, Path(temporary))
        if digest(Path(temporary) / report_name) != manifest['files'][report_name]:
            raise ReleaseError('Signed smoke evidence mismatch.')
        binary_digest = json.loads((Path(temporary) / report_name).read_text())['binary_sha256']
        cached = root / 'bin/fluxcope'
        if cached.is_symlink() or not cached.is_file() or digest(cached) != binary_digest:
            archive_name = f'fluxcope-{target}.tar.xz'
            archive_asset = next(a for a in release['assets'] if a['name'] == archive_name)
            download_asset(identity, archive_asset, Path(temporary))
            archive = Path(temporary) / archive_name
            provenance(archive, identity, bundle=Path(temporary) / BUNDLE)
            extracted = Path(temporary) / 'fluxcope'
            extract_binary(archive, extracted)
            if digest(extracted) != binary_digest:
                raise ReleaseError('Extracted executable differs from verified preview evidence.')
            os.replace(extracted, cached)
        cached.chmod(0o700)
    config = root / 'home/.fluxcope/config.yml'
    if config.is_symlink():
        raise ReleaseError('Preview config must not be a symlink.')
    if not config.exists():
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            port = listener.getsockname()[1]
        with config.open('x') as stream:
            stream.write(f'server:\n  port: {port}\n')
        config.chmod(0o600)
        print(f'Preview proxy port: {port}')
    else:
        print(f'Reusing preview port/settings from {config}')
    print(f"Running preview {identity['id']} at {identity['source']}\nState: {root / 'home'}")
    home = root / 'home'
    environment = {**os.environ, 'HOME': str(home), 'XDG_CONFIG_HOME': str(home / '.config'),
                   'XDG_DATA_HOME': str(home / '.local/share'), 'XDG_CACHE_HOME': str(home / '.cache')}
    for key in ('GH_TOKEN', 'GITHUB_TOKEN', 'GIT_TOKEN'):
        environment.pop(key, None)
    result = subprocess.run([str(root / 'bin/fluxcope')], cwd=home, env=environment)
    if result.returncode:
        raise ReleaseError('Preview exited unsuccessfully. Check its displayed error; an occupied port can be changed in its isolated config.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    publish_parser = sub.add_parser('publish')
    publish_parser.add_argument('profile', nargs='?', choices=(*PROFILES, 'all'), default='macos-arm64')
    publish_parser.add_argument('--new', action='store_true')
    for name in ('status', 'retry', 'run'):
        child = sub.add_parser(name)
        child.add_argument('id', nargs=None if name == 'retry' else '?')
    args = parser.parse_args()
    verify_local_repository()
    if args.command == 'publish':
        publish(args.profile, args.new)
    else:
        current = select_run(args.id, runnable=args.command == 'run')
        {'status': lambda r: status(r['id']), 'retry': retry, 'run': execute}[args.command](current)


if __name__ == '__main__':
    try:
        main()
    except (ReleaseError, OSError, ValueError, KeyError) as error:
        print(f'Preview: {safe_text(error)}', file=sys.stderr)
        sys.exit(1)
    except KeyboardInterrupt:
        print('\nLocal wait stopped. Remote work continues; use just preview-status.', file=sys.stderr)
        sys.exit(130)
