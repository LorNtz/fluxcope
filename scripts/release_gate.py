#!/usr/bin/env python3
"""Read-only identity gates shared by publishing, distribution, status and recovery."""
from __future__ import annotations
import argparse
import json
import os
from pathlib import Path
import re
import time
import tomllib
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from release_support import (BASE, CONFIG, REPO, SHA, ReleaseError, api, eligible_pr,
                             file_at, output, pr_intent, repository_path, run_jobs, version)


class SourceChecksPending(ReleaseError):
    """The exact source CI has not finished; not a failed check."""


def package_identity(manifest: bytes, lock: bytes) -> tuple[str, str]:
    package = tomllib.loads(manifest.decode())['package']
    if package['name'] != CONFIG['crate']:
        raise ReleaseError('Manifest has the wrong crate identity.')
    value = version(package['version'])
    matches = [p for p in tomllib.loads(lock.decode())['package'] if p['name'] == CONFIG['crate']]
    if len(matches) != 1 or matches[0]['version'] != value or 'source' in matches[0]:
        raise ReleaseError('Cargo.lock root package does not match Cargo.toml.')
    return package['name'], value


def optional_api(path: str):
    try:
        return api(repository_path(path))
    except ReleaseError as error:
        # gh records the actual HTTP response code in stderr. Network/auth/rate
        # limit failures are deliberately not interpreted as missing objects.
        if '(HTTP 404)' in str(error):
            return None
        raise


def resolve_tag(value: str) -> str | None:
    reference = optional_api(f'git/ref/tags/v{version(value)}')
    if reference is None:
        return None
    obj = reference['object']
    for _ in range(5):
        if obj['type'] == 'commit' and SHA.fullmatch(obj['sha']):
            return obj['sha']
        if obj['type'] != 'tag':
            break
        obj = api(repository_path(f"git/tags/{obj['sha']}"))['object']
    raise ReleaseError('Tag does not resolve to one valid commit.')


def registry_version(value: str) -> dict | None:
    request = Request(f"https://crates.io/api/v1/crates/{CONFIG['crate']}/{version(value)}",
                      headers={'User-Agent': f'fluxcope-release ({REPO})', 'Accept': 'application/json'})
    try:
        with urlopen(request, timeout=30) as response:
            result = json.load(response)['version']
    except HTTPError as error:
        if error.code == 404:
            return None
        raise
    if result['num'] != value or result['crate'] != CONFIG['crate'] or result['yanked']:
        raise ReleaseError('Registry identity mismatch or yanked version.')
    if not re.fullmatch('[0-9a-f]{64}', result['checksum']):
        raise ReleaseError('Registry returned an invalid crate checksum.')
    return result


def merged_release(source: str, *, number: int | None = None, expected: str | None = None) -> dict:
    if not SHA.fullmatch(source):
        raise ReleaseError('Release source must be a full commit SHA.')
    associated = api(repository_path(f'commits/{source}/pulls?per_page=100'))
    candidates = ([api(repository_path(f'pulls/{number}'))] if number else associated)
    candidates = [p for p in candidates if p['merged_at'] and p['merge_commit_sha'] == source and eligible_pr(p)]
    if len(candidates) != 1:
        raise ReleaseError('Source must be the exact merge commit of one eligible release PR.')
    pr = candidates[0]
    if len(api(repository_path(f'git/commits/{source}'))['parents']) != 1:
        raise ReleaseError('Release authorization requires a squash merge with one parent.')
    intent = pr_intent(pr)
    source_intent = json.loads(file_at('.github/release-intent.json', source))
    if source_intent != intent:
        raise ReleaseError('Merged release intent differs from the reviewed PR head.')
    _, value = package_identity(file_at('Cargo.toml', source), file_at('Cargo.lock', source))
    _, head_value = package_identity(file_at('Cargo.toml', pr['head']['sha']), file_at('Cargo.lock', pr['head']['sha']))
    if value != head_value or intent['version'] != value or (expected and value != expected):
        raise ReleaseError('Reviewed version, merged version and requested version disagree.')
    if intent['channel'] == 'stable':
        native = [p for p in associated if p['head']['ref'].startswith('release-plz-')]
        if len(native) != 1 or native[0]['number'] != pr['number']:
            raise ReleaseError('Native release-plz PR selection is ambiguous.')
        if pr['head']['sha'] != source:
            ancestry = api(repository_path(f"compare/{pr['head']['sha']}...{source}"))
            if ancestry['status'] in ('ahead', 'identical'):
                raise ReleaseError('PR head is in release ancestry; native publisher could change the authorized checkout.')
    comparison = api(repository_path(f'compare/{source}...{BASE}'))
    if comparison['status'] not in ('ahead', 'identical'):
        raise ReleaseError('Authorized source is not an ancestor of the release base.')
    return {**intent, 'source': source, 'pr': pr['number'], 'head': pr['head']['sha'], 'url': pr['html_url']}


def source_checks(source: str, *, wait: bool = False) -> int:
    workflow = api(repository_path('actions/workflows/ci.yml'))
    deadline = time.monotonic() + (1800 if wait else 0)
    while True:
        runs = api(repository_path(f"actions/workflows/{workflow['id']}/runs?head_sha={source}&event=push&per_page=100"))['workflow_runs']
        matches = [r for r in runs if r['head_sha'] == source and r['head_branch'] == BASE
                   and r['repository']['full_name'] == REPO and r['workflow_id'] == workflow['id']]
        latest = max(matches, key=lambda r: (r['run_number'], r['run_attempt']), default=None)
        if latest and latest['status'] == 'completed':
            if latest['conclusion'] != 'success':
                raise ReleaseError(f"Source CI did not pass: {latest['html_url']}")
            jobs = run_jobs(latest['id'], latest['run_attempt']).values()
            for required in CONFIG['source_checks']:
                checks = [j for j in jobs if j['name'] == required and j['head_sha'] == source]
                if len(checks) != 1 or checks[0]['conclusion'] != 'success':
                    raise ReleaseError(f'Missing successful exact-source check: {required}')
            return latest['id']
        if time.monotonic() >= deadline:
            raise SourceChecksPending('Exact-source push CI has not completed successfully yet.')
        time.sleep(10)


def dispatch_gate(tag: str, *, wait: bool = True) -> dict:
    if not tag.startswith('v'):
        raise ReleaseError('Distribution tag must begin with v.')
    value = version(tag[1:])
    source = resolve_tag(value)
    if source is None:
        raise ReleaseError('Tag must exist before dispatch; distribution never creates tags.')
    if os.environ.get('GITHUB_REF') != f'refs/tags/{tag}' or os.environ.get('GITHUB_SHA') != source:
        raise ReleaseError('Dispatch ref, tag and source SHA disagree.')
    result = merged_release(source, expected=value)
    result['source_ci_run'] = source_checks(source, wait=wait)
    if result['channel'] == 'stable':
        registry = registry_version(value)
        if registry is None:
            raise ReleaseError('Stable distribution requires the crate to be published first.')
        result['crate_sha256'] = registry['checksum']
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('source', 'distribution', 'preview'))
    parser.add_argument('--tag', default='')
    parser.add_argument('--source', default=os.environ.get('GITHUB_SHA', ''))
    parser.add_argument('--pr', type=int)
    parser.add_argument('--report', type=Path, default=Path('target/release-identity.json'))
    args = parser.parse_args()
    if os.environ.get('GITHUB_REPOSITORY') != REPO:
        raise ReleaseError('Workflow repository is not the configured release repository.')
    if args.mode == 'preview':
        _, value = package_identity(Path('Cargo.toml').read_bytes(), Path('Cargo.lock').read_bytes())
        result = {'version': value, 'source': args.source, 'channel': 'preview'}
    elif args.mode == 'distribution':
        result = dispatch_gate(args.tag)
    else:
        if os.environ.get('GITHUB_REF') != f'refs/heads/{BASE}':
            raise ReleaseError('Source publisher only accepts the base branch.')
        result = merged_release(args.source, number=args.pr)
        result['source_ci_run'] = source_checks(args.source, wait=True)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(result, indent=2) + '\n')
    for name in ('version', 'source', 'channel', 'pr'):
        if name in result:
            output(name, str(result[name]))
    output('matrix', {'include': CONFIG['platforms']})
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
