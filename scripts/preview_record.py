"""Owned preview checks and cleanup tombstones, separate from stable release results."""
from __future__ import annotations

from datetime import datetime, timezone
from functools import lru_cache
import json
import os
import re
import tempfile
from pathlib import Path

from preview_artifacts import download_public, release_for
from preview_identity import BASE, REPO, identity_from_run, trusted_run, verify_controller
from release_support import ReleaseError, api, repository_path, run_jobs


def owned_check(identity: dict) -> dict | None:
    data = api(repository_path(f"commits/{identity['source']}/check-runs?check_name=Preview%20{identity['id']}&per_page=100"))
    matches = [c for c in data['check_runs'] if c['app']['slug'] == 'github-actions'
               and c['name'] == f"Preview {identity['id']}" and c['head_sha'] == identity['source']
               and c.get('external_id') == f"fluxcope-preview:{identity['id']}"]
    if len(matches) > 1:
        raise ReleaseError('Ambiguous owned preview result checks.')
    return matches[0] if matches else None


@lru_cache(maxsize=64)
def trusted_writer(identifier: int) -> dict:
    writer = api(repository_path(f'actions/runs/{identifier}'))
    if (writer['repository']['full_name'] != REPO or writer['head_branch'] != BASE
            or writer.get('path') not in ('.github/workflows/preview.yml', '.github/workflows/preview-cleanup.yml')
            or writer['event'] not in ('workflow_dispatch', 'schedule')):
        raise ReleaseError('Preview result was not written by a trusted workflow.')
    verify_controller(writer['head_sha'])
    return writer


def read_record(identity: dict) -> dict | None:
    check = owned_check(identity)
    if check is None:
        return None
    record = json.loads(check['output'].get('text') or '{}')
    if any(record.get(key) != value for key, value in identity.items()):
        raise ReleaseError('Owned preview check has conflicting identity.')
    writer = trusted_writer(int(record['writer_run']))
    # GitHub rewrites details_url for Actions-owned checks. These records are
    # status hints; signed public assets remain the authority for execution.
    if record.get('cleanup'):
        if (writer['path'] != '.github/workflows/preview-cleanup.yml'
                or (record['cleanup'], record.get('state')) not in
                (('pending', 'cleanup-pending'), ('complete', 'expired'))):
            raise ReleaseError('Invalid preview cleanup record.')
    elif (writer['id'] != identity['id'] or writer['path'] != '.github/workflows/preview.yml'
          or writer['event'] != 'workflow_dispatch'):
        raise ReleaseError('Preview result must reference its original preview workflow.')
    return record


def write_record(identity: dict, state: str, **details):
    writer_id = int(os.environ['GITHUB_RUN_ID'])
    writer = api(repository_path(f'actions/runs/{writer_id}'))
    if (os.environ.get('GITHUB_REF') != f'refs/heads/{BASE}' or writer['head_branch'] != BASE
            or writer.get('path') not in ('.github/workflows/preview.yml', '.github/workflows/preview-cleanup.yml')):
        raise ReleaseError('Preview result writing requires trusted master workflow code.')
    record = {**identity, **details, 'state': state, 'writer_run': writer_id,
              'updated_at': datetime.now(timezone.utc).isoformat()}
    summary = (f"Preview **{identity['id']}**: **{state}** · PR #{identity['pr']} · `{identity['profile']}`\n\n"
               f"Source: `{identity['source']}`\n\n"
               f"Controller: `{identity['controller']}`\n\n"
               f"[Workflow](https://github.com/{REPO}/actions/runs/{identity['id']})")
    if details.get('url'):
        summary += f" · [Download]({details['url']})"
    payload = {'name': f"Preview {identity['id']}", 'head_sha': identity['source'],
               'external_id': f"fluxcope-preview:{identity['id']}", 'details_url': writer['html_url'],
               'status': 'completed', 'conclusion': 'success' if state == 'complete' else
               ('neutral' if state in ('expired', 'cleanup-pending', 'cancelled') else 'failure'),
               'output': {'title': f'Preview {state}', 'summary': summary, 'text': json.dumps(record, sort_keys=True)}}
    check = owned_check(identity)
    if check:
        payload.pop('head_sha')
        api(repository_path(f"check-runs/{check['id']}"), method='PATCH', payload=payload)
    else:
        api(repository_path('check-runs'), method='POST', payload=payload)
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as stream:
            stream.write(summary + '\n')


def report():
    current = trusted_run(int(os.environ['GITHUB_RUN_ID']))
    identity = identity_from_run(current)
    previous = read_record(identity)
    if previous and previous.get('cleanup'):
        return  # Never overwrite an expiry tombstone on a rerun.
    jobs = run_jobs(current['id'], current['run_attempt'])
    published = release_for(identity)
    state = 'failed'
    details = {'jobs': {name: job['conclusion'] for name, job in jobs.items() if name != 'Preview result'}}
    if published and not published['draft']:
        state = 'partial'
        if jobs.get('Preview publish', {}).get('conclusion') == 'success':
            with tempfile.TemporaryDirectory(prefix='fluxcope-preview-result-') as temporary:
                published, manifest = download_public(identity, Path(temporary), metadata_only=True)
            if manifest.get('format', 1) == 2:
                required = ['Preview attest', *(f'Preview client ({t})' for t in identity['targets']),
                            *(f'Preview install ({t})' for t in identity['targets'])]
                if any(jobs.get(name, {}).get('conclusion') != 'success' for name in required):
                    raise ReleaseError('Installer qualification is incomplete; preview cannot be complete.')
                details['installer'] = True
            state = 'complete'
            details.update(snapshot=manifest['snapshot'], url=published['html_url'], published_at=published['published_at'])
    elif any(job['conclusion'] == 'cancelled' for job in jobs.values()):
        state = 'cancelled'
    write_record(identity, state, **details)


if __name__ == '__main__':
    report()
