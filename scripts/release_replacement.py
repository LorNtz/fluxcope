"""Explicitly classify an incomplete release as awaiting a reviewed replacement."""
from urllib.parse import quote
from release_support import ReleaseError, api, repository_path


def recorded(identity: dict) -> dict | None:
    name = f"Release replacement {identity['version']}"
    checks = api(repository_path(f"commits/{identity['source']}/check-runs?check_name={quote(name)}&per_page=100"))['check_runs']
    matches = [c for c in checks if c['name'] == name and c['head_sha'] == identity['source']
               and c['app']['slug'] == 'github-actions' and c['conclusion'] == 'neutral'
               and c['external_id'] == f"fluxcope:replace:{identity['source']}:{identity['version']}"]
    return max(matches, key=lambda c: c['id'], default=None)


def record(identity: dict, reason: str):
    if not reason.strip() or len(reason) > 2000:
        raise ReleaseError('A public reason of 1–2000 characters is required for replacement classification.')
    from release_status import exact_runs
    import os
    active = [r for r in exact_runs(identity) if r['status'] != 'completed' and str(r['id']) != os.environ.get('GITHUB_RUN_ID')]
    if active:
        raise ReleaseError('Wait for active publication to finish before classifying it for replacement.')
    name = f"Release {identity['version']}"
    checks = api(repository_path(f"commits/{identity['source']}/check-runs?check_name={quote(name)}&per_page=100"))['check_runs']
    owned = [c for c in checks if c['name'] == name and c['app']['slug'] == 'github-actions']
    latest = max(owned, key=lambda c: c['id'], default=None)
    if not latest or latest['status'] != 'completed' or latest['conclusion'] == 'success':
        raise ReleaseError('Replacement classification requires a reported incomplete release with no active observation.')
    if recorded(identity):
        return
    api(repository_path('check-runs'), method='POST', payload={
        'name': f"Release replacement {identity['version']}", 'head_sha': identity['source'],
        'external_id': f"fluxcope:replace:{identity['source']}:{identity['version']}",
        'status': 'completed', 'conclusion': 'neutral',
        'output': {'title': 'Awaiting a reviewed replacement; original release remains incomplete', 'summary': reason.strip()}})
