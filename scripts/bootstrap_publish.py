#!/usr/bin/env python3
"""One-time interactive crate creation from a verified clean release source."""
from __future__ import annotations
import argparse
import getpass
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

from release_evidence import source_package
from release_gate import registry_version, resolve_tag
from release_source import check_registry
from release_status import authorized_version
from release_support import CONFIG, REPO, ReleaseError, api, repository_path, run, version


def publish(value: str, number: int):
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        raise ReleaseError('Bootstrap requires your interactive terminal; tokens are never accepted as command arguments.')
    identity = authorized_version(value, number)
    if identity['channel'] != 'stable':
        raise ReleaseError('A first crate upload must be an authorized stable release.')
    flag = api(repository_path('actions/variables/CRATES_BOOTSTRAPPED'))['value']
    if flag != 'false':
        raise ReleaseError('Bootstrap is allowed only while CRATES_BOOTSTRAPPED is explicitly false.')
    expected, artifact = source_package(identity)
    published = registry_version(value)
    if published:
        check_registry(identity, expected['sha256'])
        print(f'This exact crate is already published. Continue with just release-recover {value}.')
        return
    from release_prepare import previous_stable
    if previous_stable() is not None:
        raise ReleaseError('The crate already exists at another stable version; configure Trusted Publishing instead of repeating bootstrap.')
    if resolve_tag(value) is not None:
        raise ReleaseError('An unpublished first crate unexpectedly already has a tag; inspect before uploading.')
    if not artifact:
        raise ReleaseError('The original CI package expired. Rerun its exact source CI before bootstrap.')
    print(f"Repository: {REPO}\nRelease PR: {identity['url']}\nSource: {identity['source']}\n"
          f"Source CI: https://github.com/{REPO}/actions/runs/{expected['ci_run']}\n"
          f"Expected .crate SHA-256: {expected['sha256']}")
    with tempfile.TemporaryDirectory(prefix='fluxcope-bootstrap-') as temporary:
        checkout = Path(temporary) / 'source'
        run('git', 'clone', '--no-checkout', f'git@github.com:{REPO}.git', str(checkout), capture=False)
        run('git', 'checkout', '--detach', identity['source'], cwd=checkout, capture=False)
        run('python3', 'scripts/package_check.py', '--install', cwd=checkout, capture=False)
        actual = json.loads((checkout / 'target/package-report.json').read_text())
        if (actual['sha256'] != expected['sha256'] or actual['source'] != identity['source'] or actual['dirty']
                or actual['version'] != value):
            raise ReleaseError('Clean local package differs from exact-source CI; no token was requested or used.')
        print('The clean package and exact-package installation match CI. This upload permanently creates the registry version.')
        if input(f'Publish fluxcope {value} from this source? Type the exact version: ').strip() != value:
            raise ReleaseError('Bootstrap cancelled; no upload occurred.')
        token = getpass.getpass('One-time crates.io token (hidden; never stored): ').strip()
        if not token:
            raise ReleaseError('No token supplied.')
        try:
            environment = {k: v for k, v in os.environ.items() if k not in ('RUST_LOG', 'CARGO_LOG', 'CARGO_HTTP_DEBUG')}
            environment['CARGO_REGISTRY_TOKEN'] = token
            completed = subprocess.run(['cargo', 'publish', '--locked', '--registry', 'crates-io'], cwd=checkout, env=environment)
            if completed.returncode:
                raise ReleaseError('Cargo did not confirm the upload. Check registry facts before retrying; a lost response is not proof of failure.')
            check_registry(identity, expected['sha256'], wait=True)
        finally:
            token = ''
            if 'environment' in locals():
                environment.pop('CARGO_REGISTRY_TOKEN', None)
            print('\nRevoke the one-time token in crates.io now, including after failure or interruption.')
    print(f'Exact crate verified. Configure the Trusted Publisher, then run: just release-recover {value}')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('version')
    parser.add_argument('pr', type=int)
    args = parser.parse_args()
    try:
        publish(version(args.version), args.pr)
    except (ReleaseError, OSError, ValueError, KeyError) as error:
        print(f'Bootstrap: {error}', file=sys.stderr)
        sys.exit(1)
