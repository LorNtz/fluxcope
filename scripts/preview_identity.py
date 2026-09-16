"""Preview identities and deterministic metadata snapshots; never execute source code."""
from __future__ import annotations

import copy
from datetime import datetime, timezone
import hashlib
from functools import lru_cache
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import tomllib

from release_support import (BASE, CONFIG, REPO, ROOT, SHA, ReleaseError, api,
                             output, repository_path, run, run_jobs)

WORKFLOW = 'preview.yml'
SIGNER = f'{REPO}/.github/workflows/{WORKFLOW}'
RETENTION_DAYS = 30
PROFILES = dict(zip(('macos-arm64', 'macos-intel', 'linux-arm64', 'linux-x64'),
                    (p['target'] for p in CONFIG['platforms'])))
TITLE = re.compile(r'Preview #([1-9][0-9]*) ([0-9a-f]{40}) (macos-arm64|macos-intel|linux-arm64|linux-x64|all)')
TAG = re.compile(r'v0\.0\.0-preview\.([1-9][0-9]*)')


def preview_id(value: object) -> int:
    if not re.fullmatch(r'[1-9][0-9]{0,19}', str(value)):
        raise ReleaseError('Preview ID must be a positive workflow run ID.')
    return int(value)


def platforms(profile: str) -> list[dict]:
    if profile not in (*PROFILES, 'all'):
        raise ReleaseError(f'Unknown preview profile: {profile}')
    return [dict(p) for p in CONFIG['platforms'] if profile == 'all' or p['target'] == PROFILES[profile]]


def timestamp(value: str) -> datetime:
    parsed = datetime.fromisoformat(value.replace('Z', '+00:00'))
    if parsed.tzinfo is None:
        raise ReleaseError('Preview timestamps must include a timezone.')
    return parsed.astimezone(timezone.utc)


def validate_pr(pr: dict, source: str, *, exact: bool, opened: bool = True):
    branch = pr['head']['ref']
    if (pr['base']['ref'] != BASE or pr['base']['repo']['full_name'] != REPO
            or (pr['head'].get('repo') or {}).get('full_name') != REPO
            or branch == BASE or branch.startswith(('release-plz-', 'rc/', 'preview/', 'refs/'))
            or (opened and (pr['state'] != 'open' or pr.get('merged_at')))
            or (exact and pr['head']['sha'] != source)):
        raise ReleaseError('Preview requires an open same-repository development PR to master at the approved head.')


@lru_cache(maxsize=1)
def workflow_id() -> int:
    return api(repository_path(f'actions/workflows/{WORKFLOW}'))['id']


@lru_cache(maxsize=64)
def verify_controller(controller: str):
    comparison = api(repository_path(f'compare/{controller}...{BASE}'))
    if comparison['status'] not in ('ahead', 'identical'):
        raise ReleaseError('Preview controller is outside protected master history.')


def trusted_run(run_id: int) -> dict:
    current = api(repository_path(f'actions/runs/{preview_id(run_id)}'))
    if (current['repository']['full_name'] != REPO or current['event'] != 'workflow_dispatch'
            or current['head_branch'] != BASE or current['workflow_id'] != workflow_id()
            or current.get('path') != f'.github/workflows/{WORKFLOW}'
            or not SHA.fullmatch(current['head_sha']) or not TITLE.fullmatch(current['display_title'])):
        raise ReleaseError('Run is not an authorized master preview workflow.')
    verify_controller(current['head_sha'])
    return current


def identity_from_run(current: dict) -> dict:
    match = TITLE.fullmatch(current['display_title'])
    if not match:
        raise ReleaseError('Malformed preview run identity.')
    identifier = preview_id(current['id'])
    return {'schema': 1, 'channel': 'preview', 'id': identifier,
            'version': f'0.0.0-preview.{identifier}', 'tag': f'v0.0.0-preview.{identifier}',
            'repository': REPO, 'pr': int(match[1]), 'source': match[2],
            'controller': current['head_sha'], 'profile': match[3],
            'targets': [p['target'] for p in platforms(match[3])],
            'actor': current['actor']['login'], 'created_at': current['created_at'],
            'retention_days': RETENTION_DAYS}


def authorize(*, exact: bool = False, opened: bool = True) -> tuple[dict, dict]:
    if (os.environ.get('GITHUB_REPOSITORY') != REPO or os.environ.get('GITHUB_REF') != f'refs/heads/{BASE}'
            or os.environ.get('GITHUB_EVENT_NAME') != 'workflow_dispatch'):
        raise ReleaseError('Preview authorization must run on protected master via workflow_dispatch.')
    current = trusted_run(preview_id(os.environ['GITHUB_RUN_ID']))
    identity = identity_from_run(current)
    if identity['controller'] != os.environ.get('GITHUB_SHA'):
        raise ReleaseError('Controller checkout does not match the workflow identity.')
    if not api(repository_path(f'branches/{BASE}'))['protected']:
        raise ReleaseError('Preview publication requires protected master.')
    for actor in {current['actor']['login'], current['triggering_actor']['login']}:
        permission = api(repository_path(f'collaborators/{actor}/permission'))
        if permission.get('role_name') not in ('maintain', 'admin'):
            raise ReleaseError('Only repository maintainers/admins can request or rerun previews.')
    if exact and current['run_attempt'] > 1:
        prior = run_jobs(current['id'], current['run_attempt'] - 1).get('Preview gate', {})
        exact = prior.get('conclusion') != 'success'
    validate_pr(api(repository_path(f"pulls/{identity['pr']}")), identity['source'], exact=exact, opened=opened)
    return identity, current


def git(*arguments: str, data: bytes | None = None, env: dict | None = None, cwd: Path = ROOT) -> bytes:
    result = subprocess.run(['git', '-c', 'core.hooksPath=/dev/null', *arguments], cwd=cwd,
                            input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
    if result.returncode:
        raise ReleaseError('Snapshot Git operation failed: ' + result.stderr.decode(errors='replace'))
    return result.stdout


def blob(source: str, path: str) -> bytes:
    entry = git('ls-tree', source, '--', path).decode().strip().split()
    if len(entry) != 4 or entry[0] != '100644' or entry[1] != 'blob' or entry[3] != path:
        raise ReleaseError(f'Preview requires a regular tracked {path}.')
    if int(git('cat-file', '-s', entry[2])) > 2 * 1024 * 1024:
        raise ReleaseError(f'Preview input {path} is too large.')
    return git('cat-file', 'blob', entry[2])


def overlay(manifest: bytes, lock: bytes, identity: dict) -> dict[str, bytes]:
    original = tomllib.loads(manifest.decode())
    package = original.get('package', {})
    if (package.get('name') != CONFIG['crate'] or not isinstance(package.get('version'), str)
            or package.get('repository') != f'https://github.com/{REPO}' or 'workspace' in original
            or 'dist' in original or 'dist' in package.get('metadata', {})):
        raise ReleaseError('Unsupported manifest: use a single Fluxcope package without package-level dist overrides.')
    old_version = package['version']
    value = identity['version']
    text = manifest.decode()
    section = re.search(r'(?ms)^\[package\]\s*\n(.*?)(?=^\[|\Z)', text)
    if not section:
        raise ReleaseError('Package must use an explicit [package] table.')
    changed, count = re.subn(r'(?m)^version\s*=\s*"[^"\n]+"', f'version = "{value}"', section[1])
    if count != 1:
        raise ReleaseError('Package version must be a single explicit string.')
    updated = text[:section.start(1)] + changed + text[section.end(1):]
    expected = copy.deepcopy(original)
    expected['package']['version'] = value
    if tomllib.loads(updated) != expected:
        raise ReleaseError('Preview overlay changed non-version package metadata.')
    lock_data = tomllib.loads(lock.decode())
    roots = [p for p in lock_data['package'] if p['name'] == CONFIG['crate'] and 'source' not in p]
    if len(roots) != 1 or roots[0]['version'] != old_version:
        raise ReleaseError('Lockfile root does not match the package.')
    expected_lock = copy.deepcopy(lock_data)
    next(p for p in expected_lock['package'] if p['name'] == CONFIG['crate'] and 'source' not in p)['version'] = value
    chunks = re.split(r'(?m)(?=^\[\[package\]\])', lock.decode())
    changes = 0
    for index, chunk in enumerate(chunks):
        if not chunk.startswith('[[package]]'):
            continue
        item = tomllib.loads(chunk)['package'][0]
        if item['name'] == CONFIG['crate'] and 'source' not in item:
            chunks[index], count = re.subn(r'(?m)^version = "[^"\n]+"', f'version = "{value}"', chunk)
            changes += count
    updated_lock = ''.join(chunks)
    if changes != 1 or tomllib.loads(updated_lock) != expected_lock:
        raise ReleaseError('Preview overlay changed dependencies or ambiguous lockfile metadata.')
    policy = ('[workspace]\nmembers = ["cargo:."]\n\n[dist]\n'
              f'cargo-dist-version = "{CONFIG["tools"]["cargo-dist"]}"\n'
              'ci = []\nhosting = ["github"]\ninstallers = []\n'
              f'targets = {json.dumps(identity["targets"])}\n'
              'install-updater = false\nsource-tarball = false\nchecksum = "sha256"\n'
              '\n[dist.min-glibc-version]\n"*" = "2.28"\n')
    return {'Cargo.toml': updated.encode(), 'Cargo.lock': updated_lock.encode(), 'dist-workspace.toml': policy.encode()}


def snapshot(identity: dict, *, checkout: Path | None = None) -> dict:
    source = identity['source']
    if not SHA.fullmatch(source):
        raise ReleaseError('Source must be a full Git commit.')
    git('fetch', '--no-tags', 'origin', source)
    files = overlay(blob(source, 'Cargo.toml'), blob(source, 'Cargo.lock'), identity)
    blob(source, 'dist-workspace.toml')  # Reject links before replacing the policy.
    date = timestamp(identity['created_at']).isoformat()
    with tempfile.TemporaryDirectory(prefix='fluxcope-preview-index-') as directory:
        environment = {**os.environ, 'GIT_INDEX_FILE': str(Path(directory) / 'index'),
                       'GIT_AUTHOR_NAME': 'Fluxcope Preview', 'GIT_COMMITTER_NAME': 'Fluxcope Preview',
                       'GIT_AUTHOR_EMAIL': 'preview@fluxcope.invalid', 'GIT_COMMITTER_EMAIL': 'preview@fluxcope.invalid',
                       'GIT_AUTHOR_DATE': date, 'GIT_COMMITTER_DATE': date}
        git('read-tree', source, env=environment)
        for name, data in files.items():
            sha = git('hash-object', '-w', '--stdin', data=data).decode().strip()
            git('update-index', '--add', '--cacheinfo', f'100644,{sha},{name}', env=environment)
        tree = git('write-tree', env=environment).decode().strip()
        sha = git('commit-tree', tree, '-p', source, data=f"Preview {identity['id']}\n".encode(), env=environment).decode().strip()
    result = {**identity, 'snapshot': sha, 'tree': tree,
              'overlay': {name: hashlib.sha256(data).hexdigest() for name, data in files.items()}}
    if checkout is not None:
        git('worktree', 'add', '--detach', str(checkout.resolve()), sha)
    return result


def write_identity(identity: dict, path: Path):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(identity, sort_keys=True, indent=2) + '\n')
    for key in ('id', 'version', 'tag', 'source', 'controller', 'snapshot', 'profile'):
        output(key, identity[key])
    output('matrix', {'include': platforms(identity['profile'])})
