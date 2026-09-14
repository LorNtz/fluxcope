#!/usr/bin/env python3
"""Coordinate exact-source publishing; only explicit mutation modes write remotely."""
from __future__ import annotations
import argparse
import json
import os
from pathlib import Path
import time

from release_evidence import source_package
from release_gate import merged_release, optional_api, registry_version, resolve_tag
from release_support import (BASE, REPO, ROOT, ReleaseError, api, dispatch, output,
                             release_branch, repository_path, run, version)


def ensure_tag(identity: dict, *, token: str | None = None):
    existing = resolve_tag(identity['version'])
    if existing is not None and existing != identity['source']:
        raise ReleaseError('Existing tag points to another source; it will never be moved.')
    if existing is None:
        api(repository_path('git/refs'), method='POST', token=token,
            payload={'ref': f"refs/tags/v{identity['version']}", 'sha': identity['source']})
    if resolve_tag(identity['version']) != identity['source']:
        raise ReleaseError('Tag creation was not confirmed.')


def dispatch_distribution(identity: dict, *, operation: str = 'distribute'):
    value, source = identity['version'], identity['source']
    if resolve_tag(value) != source:
        raise ReleaseError('Cannot dispatch distribution without the exact authorized tag.')
    release = optional_api(f'releases/tags/v{value}')
    if release and not release['draft'] and operation != 'verify':
        raise ReleaseError('GitHub is already published; use tap recovery or report refresh.')
    workflow = api(repository_path('actions/workflows/release.yml'))
    def matches():
        runs = api(repository_path(f"actions/workflows/{workflow['id']}/runs?head_sha={source}&event=workflow_dispatch&per_page=100"))['workflow_runs']
        return [r for r in runs if r['head_sha'] == source and r['head_branch'] == f'v{value}'
                and r['repository']['full_name'] == REPO and r['workflow_id'] == workflow['id']]
    before = matches()
    active = [r for r in before if r['status'] != 'completed']
    if active:
        print(active[0]['html_url'])
        return
    known = {r['id'] for r in before}
    dispatch('release.yml', f'v{value}', {'tag': f'v{value}', 'operation': operation})
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        started = [r for r in matches() if r['id'] not in known]
        if started:
            print(started[0]['html_url'])
            return
        time.sleep(5)
    raise ReleaseError('Distribution dispatch was accepted but no exact-tag run appeared; inspect/recover this version.')


def check_registry(identity: dict, checksum: str, *, wait: bool = False):
    deadline = time.monotonic() + (180 if wait else 0)
    while True:
        published = registry_version(identity['version'])
        if published:
            if published['checksum'] != checksum:
                raise ReleaseError('Published crate checksum conflicts with the verified source package.')
            return
        if time.monotonic() >= deadline:
            raise ReleaseError('The exact crate version is not yet published.')
        time.sleep(5)


def route(source: str):
    if os.environ.get('GITHUB_REF') != f'refs/heads/{BASE}':
        raise ReleaseError('Source coordination requires the configured base ref.')
    if os.environ['GITHUB_EVENT_NAME'] == 'workflow_dispatch':
        output('action', 'prepare')
        return
    associated = api(repository_path(f'commits/{source}/pulls?per_page=100'))
    merged = [p for p in associated if p['merged_at'] and p['merge_commit_sha'] == source and release_branch(p['head']['ref'])]
    if not merged:
        output('action', 'prepare')
        return
    identity = merged_release(source)
    bootstrap = os.environ.get('CRATES_BOOTSTRAPPED')
    if identity['channel'] == 'stable' and bootstrap not in ('true', 'false'):
        raise ReleaseError('Set CRATES_BOOTSTRAPPED explicitly to false (first upload) or true (Trusted Publishing).')
    output('action', 'source')
    output('bootstrap', 'true' if bootstrap == 'false' and identity['channel'] == 'stable' else 'false')
    for key in ('source', 'version', 'channel', 'pr'):
        output(key, str(identity[key]))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('route', 'preflight', 'publish', 'tag', 'dispatch'))
    parser.add_argument('--source', default=os.environ.get('GITHUB_SHA', ''))
    parser.add_argument('--version')
    args = parser.parse_args()
    if os.environ.get('GITHUB_REPOSITORY') != REPO:
        raise ReleaseError('Wrong workflow repository.')
    if args.mode == 'route':
        route(args.source)
        return
    identity = merged_release(args.source, expected=version(args.version) if args.version else None)
    report, _ = source_package(identity, wait=True)
    if args.mode == 'preflight':
        destination = ROOT / 'target/source-evidence.json'
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(json.dumps({**identity, 'package': report}, indent=2) + '\n')
        print(destination.read_text())
    elif args.mode == 'publish':
        if identity['channel'] != 'stable' or os.environ.get('CRATES_BOOTSTRAPPED') != 'true':
            raise ReleaseError('Native registry publication requires a stable, bootstrapped release.')
        if os.environ.get('CARGO_REGISTRY_TOKEN') or run('git', 'rev-parse', 'HEAD') != identity['source']:
            raise ReleaseError('Publisher requires native Trusted Publishing and the exact source checkout.')
        # Native release-plz needs a branch; anchor this disposable local branch
        # to the authorized squash SHA, even when remote master has advanced.
        run('git', 'switch', '-C', BASE, identity['source'])
        run('python3', 'scripts/package_check.py', capture=False)
        rebuilt = json.loads((ROOT / 'target/package-report.json').read_text())
        if rebuilt['sha256'] != report['sha256'] or rebuilt['source'] != identity['source'] or rebuilt['dirty']:
            raise ReleaseError('Publisher package differs from source CI.')
        if registry_version(identity['version']) is None:
            run('release-plz', 'release', capture=False)
        check_registry(identity, report['sha256'], wait=True)
        ensure_tag(identity)
    elif args.mode == 'tag':
        if identity['channel'] == 'stable':
            check_registry(identity, report['sha256'])
        ensure_tag(identity)
    else:
        if identity['channel'] == 'stable':
            check_registry(identity, report['sha256'])
        dispatch_distribution(identity)


if __name__ == '__main__':
    main()
