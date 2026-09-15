#!/usr/bin/env python3
"""Recheck immutable release installations using reviewed default-branch tooling."""
import argparse
import os
from pathlib import Path
import tempfile

from homebrew_release import install, public_metadata, publish
from registry_install import install as registry_install
from release_evidence import source_package
from release_gate import resolve_tag
from release_publish import download_public
from release_source import check_registry
from release_status import authorized_version
from release_support import BASE, CONFIG, REPO, ReleaseError, output, version


def authorize(tag: str) -> dict:
    if (os.environ.get('GITHUB_REPOSITORY') != REPO
            or os.environ.get('GITHUB_REF') != f'refs/heads/{BASE}'
            or os.environ.get('GITHUB_EVENT_NAME') != 'workflow_dispatch'):
        raise ReleaseError('Installation recovery must run trusted default-branch tooling.')
    if not tag.startswith('v'):
        raise ReleaseError('Installation recovery requires an exact version tag.')
    identity = authorized_version(version(tag[1:]))
    if identity['channel'] != 'stable' or resolve_tag(identity['version']) != identity['source']:
        raise ReleaseError('Installation recovery requires the authorized stable release tag.')
    package, _ = source_package(identity)
    check_registry(identity, package['sha256'])
    return identity


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('gate', 'homebrew', 'registry'))
    parser.add_argument('--tag', required=True)
    parser.add_argument('--commit', default='')
    parser.add_argument('--target', default='')
    args = parser.parse_args()
    identity = authorize(args.tag)
    if args.mode == 'registry':
        registry_install(identity)
    else:
        with tempfile.TemporaryDirectory(prefix='fluxcope-install-verify-') as temporary:
            directory = Path(temporary)
            if args.mode == 'gate':
                download_public(identity, directory)
                publish(identity, directory, update=False)
                output('matrix', {'include': CONFIG['platforms']})
            else:
                public_metadata(identity, directory, args.target)
                install(identity, directory, args.commit, args.target)
