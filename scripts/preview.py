#!/usr/bin/env python3
"""Publish, follow, retry and run isolated previews of committed development work."""
from __future__ import annotations

import argparse
from datetime import datetime, timedelta, timezone
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlsplit

from preview_client.state import host_target
from preview_artifacts import download_public, release_for
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
                    release, manifest = download_public(identity, Path(directory), metadata_only=True)
                print(f"Download: {release['html_url']}\nRun: just preview-run {identifier}")
                if manifest.get('format', 1) == 2:
                    print(f"Install: curl --proto '=https' --tlsv1.2 -fsSL https://github.com/{REPO}/releases/download/{identity['tag']}/fluxcope-preview-installer.sh | sh")
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


def execute(current: dict):
    from preview_client.lifecycle import install, installed, launch
    from preview_client.remote import Downloads
    from preview_client.state import State, checked
    state = State(current['id'])
    downloads = Downloads('gh')
    with state.lock():
        directory = installed(state)
        if directory is None:
            if 'display_title' not in current:
                current = trusted_run(state.id)
            record = read_record(identity_from_run(current))
            if not record or record['state'] != 'complete' or record.get('cleanup'):
                raise ReleaseError('Only a complete, unexpired preview can be installed.')
            directory = install(state, downloads)
        if not (directory / 'preview-client').exists():
            launch(state, downloads)  # Existing archive-only previews use the same verification/state code.
            return
    checked(state.launcher)
    result = subprocess.run([str(state.launcher)])
    if result.returncode:
        raise ReleaseError('Preview launcher exited unsuccessfully; see its error above.')


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
    # urllib and Go's HTTP client do not use ALL_PROXY for HTTPS themselves.
    # Normalize it for this helper and its children, preserving explicit choices.
    proxy = os.environ.get('all_proxy') or os.environ.get('ALL_PROXY')
    if proxy and urlsplit(proxy).scheme in ('http', 'https'):
        for scheme in ('http', 'https'):
            key = f'{scheme}_proxy'
            if key not in os.environ and key.upper() not in os.environ:
                os.environ[key] = proxy
    verify_local_repository()
    if args.command == 'publish':
        publish(args.profile, args.new)
    elif args.command == 'run' and args.id:
        execute({'id': preview_id(args.id)})
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
