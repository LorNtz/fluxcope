"""Signed durable results in editable notes, separate from immutable release assets."""
import base64
import hashlib
import json
from pathlib import Path
import re
import tempfile

from release_support import BASE, CONFIG, REPO, ReleaseError, api, repository_path, run

START = '<!-- fluxcope-release-result:v1 -->'
END = '<!-- /fluxcope-release-result -->'


def encode(saved: dict) -> bytes:
    # Notes are a display copy. Verification reconstructs exactly these signed bytes.
    return (json.dumps(saved, sort_keys=True, separators=(',', ':')) + '\n').encode()


def verify(saved: dict, identity: dict, release: dict, package: dict):
    if not isinstance(saved, dict) or not isinstance(saved.get('package'), dict):
        raise ReleaseError('Archived result and its package must be JSON objects.')
    if (saved.get('schema') != 1 or saved.get('source') != identity['source'] or saved.get('version') != identity['version']
            or saved.get('package', {}).get('sha256') != package['sha256']
            or saved.get('public_assets') != {a['name']: a['digest'] for a in release['assets']}
            or saved.get('state') != ('complete' if identity['channel'] == 'stable' else 'rc-complete')
            or saved.get('public_verified') is not True or not str(saved.get('observer_run', '')).isdigit()):
        raise ReleaseError('Archived result does not match the immutable release identity.')
    with tempfile.TemporaryDirectory(prefix='fluxcope-result-verification-') as temporary:
        path = Path(temporary) / 'release-archive.json'
        path.write_bytes(encode(saved))
        run('gh', 'attestation', 'verify', str(path), '--repo', REPO,
            '--signer-workflow', f'{REPO}/.github/workflows/release-status.yml',
            '--source-ref', f'refs/heads/{BASE}', '--deny-self-hosted-runners')
    if identity['channel'] == 'stable':
        tap = saved.get('homebrew')
        if not isinstance(tap, dict) or tap.get('tap') != CONFIG['tap'] or saved.get('registry_install') is not True:
            raise ReleaseError('Archived stable installation evidence is incomplete.')
        formula = api(f"repos/{CONFIG['tap']}/contents/Formula/fluxcope.rb?ref={tap['commit']}")
        digest = hashlib.sha256(base64.b64decode(formula['content'])).hexdigest()
        assets = {a['name']: a['digest'] for a in release['assets']}
        if digest != tap['formula_sha256'] or assets.get('fluxcope.rb') != 'sha256:' + digest:
            raise ReleaseError('Archived tap commit differs from the immutable formula.')


def read(identity: dict, release: dict, package: dict) -> dict | None:
    body = release.get('body') or ''
    if START not in body:
        return None
    match = re.search(re.escape(START) + r'\n```json\n(.*?)\n```\n' + re.escape(END), body, re.DOTALL)
    if not match or len(match[1]) > 60000 or body.count(START) != 1 or body.count(END) != 1:
        raise ReleaseError('Malformed archived release result.')
    saved = json.loads(match[1])
    verify(saved, identity, release, package)
    return saved


def prepare(identity: dict, report: dict, release: dict) -> dict:
    if report['state'] not in ('complete', 'rc-complete') or release['draft'] or not release.get('immutable'):
        raise ReleaseError('Only fully verified immutable releases can have a completed result signed.')
    saved = {key: report[key] for key in ('schema', 'version', 'source', 'state', 'package', 'public_verified', 'homebrew', 'registry_install')}
    saved['public_assets'] = {a['name']: a['digest'] for a in release['assets']}
    saved['raw_evidence_retention_days'] = 90
    saved['observer_run'] = report['observer_run']
    return saved


def write(identity: dict, saved: dict):
    release = api(repository_path(f"releases/tags/v{identity['version']}"))
    if release['draft'] or not release.get('immutable'):
        raise ReleaseError('Durable result requires an immutable published release.')
    verify(saved, identity, release, saved['package'])
    body = release.get('body') or ''
    if (body.count(START) != body.count(END) or body.count(START) > 1
            or (START in body and body.index(END) < body.index(START))):
        raise ReleaseError('Release notes have unmatched or duplicate result markers. Remove only the damaged fluxcope-release-result block in the GitHub notes editor, preserving the changelog, then run just release-recover again. No notes were written.')
    if START in body:
        body = re.sub(re.escape(START) + r'.*?' + re.escape(END), '', body, flags=re.DOTALL)
    body = body.rstrip() + '\n\n' + START + '\n```json\n' + json.dumps(saved, indent=2) + '\n```\n' + END + '\n'
    try:
        api(repository_path(f"releases/{release['id']}"), method='PATCH', payload={'body': body})
    except ReleaseError:
        # An API response can be lost after the write succeeded. Read the facts
        # before reporting failure; never infer success from the attempted PATCH.
        reread = api(repository_path(f"releases/{release['id']}"))
        if read(identity, reread, saved['package']) == saved:
            return
        raise
    reread = api(repository_path(f"releases/{release['id']}"))
    if read(identity, reread, saved['package']) != saved:
        raise ReleaseError('Durable result write was not confirmed.')
