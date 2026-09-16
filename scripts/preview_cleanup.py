"""Expire only verified preview downloads; retain every protected source tag."""
from __future__ import annotations

import argparse
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import tempfile

from preview_artifacts import download_public, release_for, resolve_preview_tag
from preview_identity import BASE, REPO, TAG, TITLE, identity_from_run, timestamp, trusted_run
from preview_record import read_record, write_record
from release_support import ReleaseError, api, output, pages, repository_path


def require_cleanup_context():
    current = api(repository_path(f"actions/runs/{int(os.environ['GITHUB_RUN_ID'])}"))
    if (os.environ.get('GITHUB_REPOSITORY') != REPO or os.environ.get('GITHUB_REF') != f'refs/heads/{BASE}'
            or current['head_branch'] != BASE or current['path'] != '.github/workflows/preview-cleanup.yml'
            or current['event'] not in ('workflow_dispatch', 'schedule')):
        raise ReleaseError('Cleanup requires its trusted master workflow.')
    if current['event'] == 'workflow_dispatch':
        permission = api(repository_path(f"collaborators/{current['triggering_actor']['login']}/permission"))
        if permission.get('role_name') not in ('maintain', 'admin'):
            raise ReleaseError('Only maintainers/admins can dispatch preview cleanup.')


def expired(release: dict, days: int, now: datetime) -> bool:
    created = release['created_at'] if release['draft'] else release['published_at']
    return now >= timestamp(created) + timedelta(days=days)


def discover():
    require_cleanup_context()
    now = datetime.now(timezone.utc)
    identifiers = []
    for release in pages(repository_path('releases?per_page=100')):
        match = TAG.fullmatch(release['tag_name'])
        if match and expired(release, 30, now):
            identifiers.append(int(match[1]))
    # Include interrupted deletions whose Release has disappeared. An owned
    # pending check is found through its original preview run, never a bare 404.
    page = 1
    while True:
        response = api(repository_path(f'actions/workflows/preview.yml/runs?event=workflow_dispatch&branch=master&per_page=100&page={page}'))
        for current in response['workflow_runs']:
            if not TITLE.fullmatch(current.get('display_title', '')):
                continue  # Rejected manual input must not block unrelated expiry.
            identity = identity_from_run(current)
            if now < timestamp(identity['created_at']) + timedelta(days=30):
                continue
            record = read_record(identity)
            if record and record.get('cleanup') == 'pending':
                identifiers.append(identity['id'])
        if len(response['workflow_runs']) < 100:
            break
        page += 1
    selected = sorted(set(identifiers))[:64]
    output('ids', selected)
    output('any', 'true' if selected else 'false')


def cleanup(identifier: int):
    require_cleanup_context()
    original = trusted_run(identifier)
    identity = identity_from_run(original)
    previous = read_record(identity)
    release = release_for(identity, include_drafts=True)
    if previous and previous.get('cleanup') == 'complete':
        return
    if original['status'] != 'completed':
        print('Preview is active; cleanup deferred.')
        return
    if release is None:
        if previous and previous.get('cleanup') == 'pending':
            if resolve_preview_tag(identity) != previous['snapshot']:
                raise ReleaseError('Pending cleanup lost its protected source tag.')
            write_record(identity, 'expired', cleanup='complete', snapshot=previous['snapshot'],
                         former_url=previous['former_url'], removed_at=datetime.now(timezone.utc).isoformat())
            return
        raise ReleaseError('Missing preview has no trusted cleanup intent; deletion cannot be inferred.')
    if not expired(release, identity['retention_days'], datetime.now(timezone.utc)):
        print('Preview has not expired; nothing deleted.')
        return
    if release['draft']:
        # Incomplete drafts may not yet have an attestation bundle. Require the
        # source App, exact originating run and deterministic commit envelope.
        if release['author']['login'] != 'fluxcope-release[bot]':
            raise ReleaseError('Draft was not created by the source publication App.')
        snapshot = resolve_preview_tag(identity)
        if not snapshot:
            raise ReleaseError('Preview draft has no protected snapshot tag.')
        commit = api(repository_path(f'git/commits/{snapshot}'))
        if ([p['sha'] for p in commit['parents']] != [identity['source']]
                or commit['message'].strip() != f"Preview {identity['id']}"
                or timestamp(commit['committer']['date']) != timestamp(identity['created_at'])):
            raise ReleaseError('Draft snapshot does not match its preview run.')
    else:
        with tempfile.TemporaryDirectory(prefix='fluxcope-preview-expiry-') as temporary:
            _, manifest = download_public(identity, Path(temporary), metadata_only=True, allow_expired=True)
        snapshot = manifest['snapshot']
    reread = api(repository_path(f"releases/{release['id']}"))
    if (reread['tag_name'] != identity['tag'] or reread['draft'] != release['draft']
            or not expired(reread, identity['retention_days'], datetime.now(timezone.utc))):
        raise ReleaseError('Preview state changed during cleanup.')
    write_record(identity, 'cleanup-pending', cleanup='pending', snapshot=snapshot, former_url=release['html_url'])
    api(repository_path(f"releases/{release['id']}"), method='DELETE')
    if release_for(identity, include_drafts=True) is not None or resolve_preview_tag(identity) != snapshot:
        raise ReleaseError('Preview deletion/tag preservation was not confirmed; cleanup remains pending.')
    write_record(identity, 'expired', cleanup='complete', snapshot=snapshot, former_url=release['html_url'],
                 removed_at=datetime.now(timezone.utc).isoformat())


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('discover', 'delete'))
    parser.add_argument('--id', type=int)
    args = parser.parse_args()
    discover() if args.command == 'discover' else cleanup(args.id)
