#!/usr/bin/env python3
"""Publish cargo-dist's exact formula and verify installation from its tap commit."""
from __future__ import annotations
import argparse
import base64
import hashlib
import json
import os
import re
from pathlib import Path
import tempfile
from urllib.parse import quote
from urllib.request import urlopen

from dist_artifacts import digest
from release_evidence import source_package
from release_gate import dispatch_gate, optional_api, resolve_tag
from release_source import check_registry
from release_support import CONFIG, ROOT, ReleaseError, api, git_repository, output, run

TAP = CONFIG['tap']
TAP_NAME = TAP.replace('/homebrew-', '/')


def public_metadata(identity: dict, directory: Path, target: str):
    # The workflow gate verifies all public archives and provenance once. These
    # jobs need only immutable formula/target evidence; brew downloads its native
    # archive and we compare the installed executable to the tested digest.
    release = optional_api(f"releases/tags/v{identity['version']}")
    if (not release or release['draft'] or not release.get('immutable') or release['prerelease']
            or resolve_tag(identity['version']) != identity['source']):
        raise ReleaseError('Homebrew requires the exact immutable stable release.')
    names = {'fluxcope.rb'}
    if target:
        if target not in {p['target'] for p in CONFIG['platforms']}:
            raise ReleaseError('Unsupported Homebrew target.')
        names.add(f'{target}-smoke.json')
    for name in names:
        assets = [a for a in release['assets'] if a['name'] == name]
        if len(assets) != 1 or assets[0]['size'] > 1024 * 1024:
            raise ReleaseError('Missing or invalid immutable Homebrew metadata.')
        with urlopen(f"https://github.com/{CONFIG['repository']}/releases/download/v{identity['version']}/{name}", timeout=30) as stream:
            data = stream.read(1024 * 1024 + 1)
        if len(data) > 1024 * 1024 or assets[0].get('digest') != 'sha256:' + hashlib.sha256(data).hexdigest():
            raise ReleaseError('Homebrew metadata download checksum mismatch.')
        (directory / name).write_bytes(data)
    if target:
        report = json.loads((directory / f'{target}-smoke.json').read_text())
        if (report.get('source') != identity['source'] or report.get('version') != identity['version']
                or report.get('target') != target or report.get('dirty', True) or report.get('result') != 'passed'):
            raise ReleaseError('Homebrew target evidence identity mismatch.')


def formula_at(ref: str) -> dict | None:
    try:
        result = api(f'repos/{TAP}/contents/Formula/fluxcope.rb?ref={quote(ref, safe="")}')
    except ReleaseError as error:
        if '(HTTP 404)' in str(error):
            return None
        raise
    if result.get('type') != 'file' or result.get('encoding') != 'base64':
        raise ReleaseError('Expected a regular formula in the tap.')
    return result


class FormulaUnavailable(ReleaseError):
    """The requested formula has not been published in the tap."""


def resolve_tap_commit(identity: dict, formula: bytes, *, update: bool) -> str:
    repository = api(f'repos/{TAP}')
    if repository['private']:
        raise ReleaseError('Homebrew tap must be public.')
    branch = repository['default_branch']
    before = api(f'repos/{TAP}/git/ref/heads/{quote(branch, safe="")}')['object']['sha']
    old = formula_at(before)
    commit = None
    if old:
        text = base64.b64decode(old['content']).decode()
        versions = re.findall(r'^  version "([0-9]+\.[0-9]+\.[0-9]+)"$', text, re.MULTILINE)
        if len(versions) != 1:
            raise ReleaseError('Existing formula has no unambiguous stable version.')
        if tuple(map(int, versions[0].split('.'))) > tuple(map(int, identity['version'].split('.'))):
            # Recovery of an older release tests an existing historical formula;
            # it never rolls the public tap back from a newer version.
            for page in range(1, 11):
                history = api(f'repos/{TAP}/commits?sha={before}&path=Formula/fluxcope.rb&per_page=100&page={page}')
                for entry in history:
                    historical = formula_at(entry['sha'])
                    if historical and base64.b64decode(historical['content']) == formula:
                        commit = entry['sha']
                        break
                if commit or len(history) < 100:
                    break
            if not commit:
                raise ReleaseError('No matching historical formula found; refusing to downgrade the newer tap.')
    if commit:
        pass
    elif old and base64.b64decode(old['content']) == formula:
        commit = before
    elif not update:
        raise FormulaUnavailable('The exact release formula is not yet present in the tap.')
    else:
        payload = {'message': f"chore: release fluxcope {identity['version']}",
                   'content': base64.b64encode(formula).decode(), 'branch': branch}
        if old:
            payload['sha'] = old['sha']
        result = api(f'repos/{TAP}/contents/Formula/fluxcope.rb', method='PUT', payload=payload)
        commit = result['commit']['sha']
    actual = formula_at(commit)
    if actual is None or base64.b64decode(actual['content']) != formula:
        raise ReleaseError('Tap commit does not contain the verified formula.')
    return commit


def publish(identity: dict, directory: Path, *, update: bool = True):
    formula = (directory / 'fluxcope.rb').read_bytes()
    commit = resolve_tap_commit(identity, formula, update=update)
    report = {'schema': 1, 'source': identity['source'], 'version': identity['version'],
              'tap': TAP, 'commit': commit, 'formula_sha256': hashlib.sha256(formula).hexdigest()}
    (ROOT / 'target').mkdir(exist_ok=True)
    (ROOT / 'target/tap-commit.json').write_text(json.dumps(report, indent=2) + '\n')
    output('commit', commit)
    print(json.dumps(report))


def install(identity: dict, directory: Path, commit: str, target: str):
    if os.environ.get('GITHUB_ACTIONS') != 'true':
        raise ReleaseError('Homebrew installation checks run only on disposable CI machines.')
    if target not in {p['target'] for p in CONFIG['platforms']}:
        raise ReleaseError('Unsupported Homebrew target.')
    if not re.fullmatch(r'[0-9a-f]{40}', commit):
        raise ReleaseError('Homebrew requires an exact tap commit SHA.')
    report = json.loads((directory / f'{target}-smoke.json').read_text())
    # Do not run `brew tap`: it may evaluate an unpinned formula while cloning.
    # Register the checkout in Homebrew's normal tap path, verify its exact bytes,
    # then grant trust to this formula alone before any Ruby evaluation.
    tap_path = Path(run('brew', '--repo', TAP_NAME))
    if not tap_path.exists():
        tap_path.parent.mkdir(parents=True, exist_ok=True)
        run('git', 'clone', '--no-checkout', f'https://github.com/{TAP}.git', str(tap_path), capture=False)
    origin = run('git', '-C', str(tap_path), 'remote', 'get-url', 'origin')
    if (git_repository(origin) or '').casefold() != TAP.casefold():
        raise ReleaseError('Homebrew tap checkout has an unexpected origin.')
    run('git', '-C', str(tap_path), 'fetch', 'origin', commit, capture=False)
    run('git', '-C', str(tap_path), 'checkout', '--detach', commit, capture=False)
    if run('git', '-C', str(tap_path), 'rev-parse', 'HEAD') != commit:
        raise ReleaseError('Homebrew tap checkout does not match the requested commit.')
    if (tap_path / 'Formula/fluxcope.rb').read_bytes() != (directory / 'fluxcope.rb').read_bytes():
        raise ReleaseError('Tap checkout differs from the immutable formula snapshot.')
    run('brew', 'trust', '--formula', f'{TAP_NAME}/fluxcope', capture=False)
    run('brew', 'install', f'{TAP_NAME}/fluxcope', capture=False)
    binary = Path(run('brew', '--prefix', f'{TAP_NAME}/fluxcope')) / 'bin/fluxcope'
    if digest(binary) != report['binary_sha256']:
        raise ReleaseError('Homebrew installed bytes other than the verified archive binary.')
    result = ROOT / f'target/homebrew-{target}.json'
    run('python3', 'scripts/smoke.py', '--binary', str(binary), '--version', identity['version'],
        '--target', target, '--report', str(result), capture=False)
    data = json.loads(result.read_text())
    data.update(source=identity['source'], tap=TAP, tap_commit=commit,
                formula_sha256=digest(directory / 'fluxcope.rb'))
    result.write_text(json.dumps(data, indent=2) + '\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('publish', 'install'))
    parser.add_argument('--tag', required=True)
    parser.add_argument('--commit', default='')
    parser.add_argument('--target', default='')
    args = parser.parse_args()
    identity = dispatch_gate(args.tag)
    if identity['channel'] != 'stable':
        raise ReleaseError('RC versions never publish or test the stable tap.')
    package, _ = source_package(identity)
    check_registry(identity, package['sha256'])
    with tempfile.TemporaryDirectory(prefix='fluxcope-homebrew-') as temporary:
        directory = Path(temporary)
        public_metadata(identity, directory, args.target)
        if args.mode == 'publish':
            publish(identity, directory)
        else:
            install(identity, directory, args.commit, args.target)
