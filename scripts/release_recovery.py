#!/usr/bin/env python3
"""Select a concrete recovery step from current facts; no new version authorization."""
from __future__ import annotations
import argparse
import json
import os
from pathlib import Path
import tempfile

from homebrew_release import FormulaUnavailable, public_metadata, resolve_tap_commit

from release_evidence import source_package
from release_gate import optional_api, registry_version, resolve_tag
from release_source import check_registry, dispatch_distribution, ensure_tag
from release_status import authorized_version, exact_runs, snapshot
from release_support import BASE, REPO, ROOT, ReleaseError, api, dispatch, output, repository_path, version

OPERATIONS = {'refresh', 'source-rerun', 'bootstrap-upload', 'tag-and-distribute', 'distribute', 'verify-public', 'stable-installations', 'verify-installations', 'wait'}


def plan(identity: dict, *, bootstrap: bool) -> dict:
    runs = exact_runs(identity)
    active = [r for r in runs if r['status'] != 'completed' and str(r['id']) != os.environ.get('GITHUB_RUN_ID')]
    if active:
        return {'operation': 'wait', 'description': 'The existing release run is still active.', 'url': active[0]['html_url']}
    package, _ = source_package(identity)
    crate = registry_version(identity['version']) if identity['channel'] == 'stable' else None
    if crate and crate['checksum'] != package['sha256']:
        raise ReleaseError('Registry integrity conflict; recovery cannot replace a crate.')
    tag = resolve_tag(identity['version'])
    if tag is not None and tag != identity['source']:
        raise ReleaseError('Tag integrity conflict; recovery cannot move a tag.')
    release = optional_api(f"releases/tags/v{identity['version']}")
    if identity['channel'] == 'stable' and not crate:
        if bootstrap:
            return {'operation': 'bootstrap-upload', 'description': 'Complete the one-time manual crate upload; no tag will be created yet.'}
        sources = [r for r in runs if r['file'] == 'release-plz.yml' and r['event'] == 'push']
        if not sources:
            raise ReleaseError('No exact-source publisher run exists to retry.')
        selected = max(sources, key=lambda r: (r['id'], r['run_attempt']))
        return {'operation': 'source-rerun', 'description': 'Retry the original source publisher at its original commit.', 'run_id': selected['id'], 'url': selected['html_url']}
    if release and not release['draft']:
        if tag is None or not release.get('immutable'):
            raise ReleaseError('Published release is missing its immutable source identity.')
        report = snapshot(identity)
        if report['state'] in ('complete', 'rc-complete'):
            return {'operation': 'refresh', 'description': 'Refresh the already completed release report without publishing.'}
        if not report.get('public_verified'):
            return {'operation': 'verify-public', 'description': 'Verify the existing immutable public assets without rebuilding or reuploading; then finish stable installation checks if applicable.'}
        if identity['channel'] == 'stable':
            with tempfile.TemporaryDirectory(prefix='fluxcope-recovery-plan-') as temporary:
                directory = Path(temporary)
                public_metadata(identity, directory, '')
                try:
                    resolve_tap_commit(identity, (directory / 'fluxcope.rb').read_bytes(), update=False)
                except FormulaUnavailable:
                    pass
                else:
                    return {'operation': 'verify-installations', 'description': 'Recheck the published registry package and exact tap commit with reviewed master tooling; no publication writes.'}
            return {'operation': 'stable-installations', 'description': 'Verify published assets, repair the personal tap if needed, and rerun exact registry/Homebrew installation checks.'}
        return {'operation': 'refresh', 'description': 'Refresh the RC result from existing public evidence.'}
    if tag is None:
        return {'operation': 'tag-and-distribute', 'description': 'Create the missing exact-source tag and start distribution of the already published crate or authorized RC.'}
    return {'operation': 'distribute', 'description': 'Build and validate this existing tag, then finish its unpublished draft and installation checks.'}


def execute(identity: dict, requested: str, actual: dict):
    if requested != actual['operation']:
        raise ReleaseError(f'Remote state changed; requested {requested}, now requires {actual["operation"]}. Inspect the new plan before retrying.')
    if requested in ('wait', 'bootstrap-upload'):
        raise ReleaseError(actual['description'] + (' ' + actual['url'] if 'url' in actual else ' See docs/releasing.md.'))
    if requested == 'source-rerun':
        api(repository_path(f"actions/runs/{actual['run_id']}/rerun"), method='POST')
    elif requested == 'tag-and-distribute':
        ensure_tag(identity, token=os.environ.get('TAG_CREATION_TOKEN'))
        dispatch_distribution(identity)
    elif requested == 'distribute':
        dispatch_distribution(identity)
    elif requested == 'verify-public':
        dispatch_distribution(identity, operation='verify')
    elif requested == 'verify-installations':
        dispatch('verify-installations.yml', BASE, {'tag': f"v{identity['version']}"})
    elif requested == 'stable-installations':
        dispatch('publish-homebrew.yml', f"v{identity['version']}", {'tag': f"v{identity['version']}"})
    else:
        dispatch('release-status.yml', BASE, {'version': identity['version'], 'pr': str(identity['pr'])})


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('plan', 'execute'))
    parser.add_argument('--version', required=True)
    parser.add_argument('--pr', type=int, required=True)
    parser.add_argument('--operation', default='')
    parser.add_argument('--reason', default='')
    args = parser.parse_args()
    if os.environ.get('GITHUB_REPOSITORY') != REPO or os.environ.get('GITHUB_REF') != f'refs/heads/{BASE}':
        raise ReleaseError('Recovery must execute trusted base-branch control code.')
    identity = authorized_version(version(args.version), args.pr)
    if args.operation == 'mark-for-replacement':
        if not args.reason.strip():
            raise ReleaseError('A public replacement reason is required.')
        output('operation', args.operation)
        if args.mode == 'execute':
            from release_replacement import record
            record(identity, args.reason)
        raise SystemExit(0)
    selected = plan(identity, bootstrap=os.environ.get('CRATES_BOOTSTRAPPED') == 'false')
    if args.operation and args.operation != selected['operation']:
        raise ReleaseError(f'Requested recovery is stale; the current plan is: {selected}')
    output('operation', selected['operation'])
    print(json.dumps({**identity, **selected}, indent=2))
    if args.mode == 'execute':
        execute(identity, args.operation, selected)
