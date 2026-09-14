#!/usr/bin/env python3
"""Prepare one reviewable PR using release-plz version/changelog logic and an expected-head commit."""
from __future__ import annotations
import argparse
import base64
import json
import os
import tomllib
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from release_gate import optional_api, package_identity, registry_version, resolve_tag
from release_support import (BASE, CONFIG, REPO, ROOT, SHA, ReleaseError, api, eligible_pr, file_at,
                             output, pages, pr_intent, repository_path, run, validate_intent, version)

FILES = ('Cargo.toml', 'Cargo.lock', 'CHANGELOG.md', '.github/release-intent.json')


def previous_stable() -> str | None:
    request = Request(f"https://crates.io/api/v1/crates/{CONFIG['crate']}", headers={'User-Agent': f'fluxcope-release ({REPO})'})
    try:
        with urlopen(request, timeout=30) as response:
            data = json.load(response)
    except HTTPError as error:
        if error.code == 404:
            return None
        raise
    candidates = [v['num'] for v in data['versions'] if '-' not in v['num'] and not v['yanked']]
    return max(candidates, key=lambda v: tuple(map(int, version(v).split('.'))), default=None)


def prepared_tree(base: str, values: dict[str, bytes]) -> str:
    run('git', 'read-tree', base)
    for name, content in values.items():
        (ROOT / name).write_bytes(content)
    run('git', 'add', '--', *FILES)
    return run('git', 'write-tree')


def push_preparation(branch: str, head: str | None, base: str, values: dict[str, bytes], message: str) -> None:
    # The disposable index starts at the reviewed base; overlay only release
    # metadata. Both parents keep newer master work and the prior PR reachable.
    token = os.environ['GIT_TOKEN']
    authorization = base64.b64encode(f'x-access-token:{token}'.encode()).decode()
    environment = {**os.environ, 'GIT_CONFIG_COUNT': '1',
        'GIT_CONFIG_KEY_0': 'http.https://github.com/.extraheader',
        'GIT_CONFIG_VALUE_0': f'AUTHORIZATION: basic {authorization}',
        'GIT_AUTHOR_NAME': CONFIG['release_authors'][-1],
        'GIT_AUTHOR_EMAIL': f"{CONFIG['release_authors'][-1]}@users.noreply.github.com",
        'GIT_COMMITTER_NAME': CONFIG['release_authors'][-1],
        'GIT_COMMITTER_EMAIL': f"{CONFIG['release_authors'][-1]}@users.noreply.github.com"}
    if head:
        run('git', 'fetch', 'origin', head, env=environment)
    tree = prepared_tree(base, values)
    if head and tree == run('git', 'rev-parse', f'{head}^{{tree}}'):
        print('Prepared content is unchanged; keeping the existing PR head and checks.')
        return
    parents = ['-p', head] if head else ['-p', base]
    if head and head != base:
        comparison = api(repository_path(f'compare/{base}...{head}'))
        if comparison['status'] not in ('ahead', 'identical'):
            parents += ['-p', base]
    commit = run('git', 'commit-tree', tree, *parents, '-m', message, env=environment)
    # An exact lease guards concurrent maintainer edits. For an existing ref this
    # is also proven fast-forward: its old head is a parent of the new commit.
    run('git', 'push', f'--force-with-lease=refs/heads/{branch}:{head or ""}',
        'origin', f'{commit}:refs/heads/{branch}', env=environment, capture=False)


def plan(requested: str | None):
    if run('git', 'status', '--porcelain'):
        raise ReleaseError('Preparation requires a clean disposable checkout.')
    # A prior run may have created the PR but failed before adding its label.
    # Eligibility for publication still requires the real label; preparation may
    # repair only otherwise eligible same-repository/author PRs.
    opened = [p for p in pages(repository_path(f'pulls?state=open&base={BASE}&per_page=100'))
              if eligible_pr({**p, 'labels': [{'name': 'release'}]})]
    if len(opened) > 1:
        raise ReleaseError('Multiple release PRs are open; close superseded candidates before preparing.')
    current = opened[0] if opened else None
    retained = pr_intent({**current, 'labels': [{'name': 'release'}]}) if current else None
    if not requested and retained and retained['mode'] == 'explicit':
        requested = retained['version']
    predecessor = previous_stable()
    # Block a new version while a previously merged release is incomplete. The
    # status workflow is the only producer trusted to mark a release complete.
    replacement = None
    closed = pages(repository_path(f'pulls?state=closed&base={BASE}&per_page=100'))
    for pr in sorted(closed, key=lambda p: p['merged_at'] or '', reverse=True):
        if not pr['merged_at'] or not eligible_pr(pr):
            continue
        intent = pr_intent(pr)
        checks = api(repository_path(f"commits/{pr['merge_commit_sha']}/check-runs?per_page=100"))['check_runs']
        matching = [c for c in checks if c['name'] == f"Release {intent['version']}" and c['app']['slug'] == 'github-actions']
        latest = max(matching, key=lambda c: c['id'], default=None)
        if not latest or latest['conclusion'] != 'success':
            from release_replacement import recorded
            replaced = {'source': pr['merge_commit_sha'], 'version': intent['version']}
            marker = recorded(replaced)
            if not marker:
                raise ReleaseError(f"Release {intent['version']} is incomplete; recover it before preparing another version.")
            replacement = {**replaced, 'reason': marker['output']['summary']}
        break
    run('release-plz', 'update', '--repo-url', f'https://github.com/{REPO}', capture=False)
    if requested:
        run('release-plz', 'set-version', f"{CONFIG['crate']}@{requested}", capture=False)
    value = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']
    version(value)
    if predecessor and tuple(map(int, value.split('-')[0].split('.'))) <= tuple(map(int, predecessor.split('.'))):
        if not requested and value == predecessor:
            print('No unpublished version follows the previous stable release.')
            return
        raise ReleaseError('A new stable or RC version must be newer than the previous stable release.')
    if registry_version(value) is not None or resolve_tag(value) is not None:
        if not requested:
            print('No unpublished package changes require a release.')
            return
        raise ReleaseError('The requested version is already present in a publication channel.')
    if '-rc.' in value and not requested:
        raise ReleaseError('An RC base requires an explicit next RC or stable promotion version.')
    # set-version updates the manifest/changelog; refresh only workspace entries.
    run('cargo', 'update', '--workspace', capture=False)
    package_identity((ROOT / 'Cargo.toml').read_bytes(), (ROOT / 'Cargo.lock').read_bytes())
    intent = validate_intent({'schema': 1, 'channel': 'rc' if '-rc.' in value else 'stable',
                              'mode': 'explicit' if requested else 'automatic', 'version': value,
                              'previous_stable': predecessor})
    if replacement:
        intent['replaces'] = replacement
    (ROOT / '.github/release-intent.json').write_text(json.dumps(intent, indent=2) + '\n')
    changed = set(run('git', 'diff', '--name-only').splitlines())
    changed.update(run('git', 'ls-files', '--others', '--exclude-standard').splitlines())
    if not changed <= set(FILES):
        raise ReleaseError(f'Release tools changed files outside the allowed set: {sorted(changed - set(FILES))}')
    branch = f'rc/{value}' if intent['channel'] == 'rc' else f'release-plz-{value}'
    base = run('git', 'rev-parse', 'HEAD')
    old_ref = optional_api(f'git/ref/heads/{branch}')
    head = old_ref['object']['sha'] if old_ref else None
    if head:
        if current and current['head']['ref'] == branch and current['head']['sha'] != head:
            raise ReleaseError('Existing preparation branch has no matching open release PR.')
        run('git', 'fetch', 'origin', head)
        changed_paths = run('git', 'diff', '--name-only', f'{base}...{head}').splitlines()
        if any(path not in FILES for path in changed_paths):
            raise ReleaseError('Release PR includes manual source changes; review/close it before regeneration.')
        validate_intent(json.loads(file_at('.github/release-intent.json', head)), value)
    data = {'schema': 1, 'base': base, 'head': head, 'version': value,
            'current_pr': current['number'] if current else None,
            'current_head': current['head']['sha'] if current else None,
            'files': {name: base64.b64encode((ROOT / name).read_bytes()).decode() for name in FILES}}
    (ROOT / 'target').mkdir(exist_ok=True)
    (ROOT / 'target/release-preparation.json').write_text(json.dumps(data) + '\n')
    output('prepared', 'true')
    output('attempt', os.environ['GITHUB_RUN_ATTEMPT'])


def apply():
    # This job has no Cargo/tool execution and uses trusted default-branch code.
    # The read-only producer can supply only bounded release metadata, never scripts.
    path = ROOT / 'target/release-preparation.json'
    if path.stat().st_size > 4 * 1024 * 1024:
        raise ReleaseError('Preparation metadata exceeds its size limit.')
    data = json.loads(path.read_text())
    if data.get('schema') != 1 or set(data.get('files', {})) != set(FILES):
        raise ReleaseError('Invalid preparation schema or file allowlist.')
    base, head = data['base'], data['head']
    if not SHA.fullmatch(base) or (head is not None and not SHA.fullmatch(head)):
        raise ReleaseError('Preparation contains an invalid source SHA.')
    if base != run('git', 'rev-parse', 'HEAD') or base != api(repository_path(f'git/ref/heads/{BASE}'))['object']['sha']:
        raise ReleaseError('Master advanced during preparation; rerun preparation before writing a PR.')
    values = {name: base64.b64decode(content, validate=True) for name, content in data['files'].items()}
    value = version(data['version'])
    if package_identity(values['Cargo.toml'], values['Cargo.lock'])[1] != value:
        raise ReleaseError('Prepared package does not match the requested version.')
    intent = validate_intent(json.loads(values['.github/release-intent.json']), value)
    predecessor = intent['previous_stable']
    branch = f'rc/{value}' if intent['channel'] == 'rc' else f'release-plz-{value}'
    remote = optional_api(f'git/ref/heads/{branch}')
    actual = remote['object']['sha'] if remote else None
    already_applied = False
    if actual != head:
        if actual is None:
            raise ReleaseError('The preparation branch disappeared after planning.')
        run('git', 'fetch', 'origin', actual)
        if run('git', 'rev-parse', f'{actual}^{{tree}}') != prepared_tree(base, values):
            raise ReleaseError('The preparation branch changed to content outside this plan.')
        run('git', 'merge-base', '--is-ancestor', base, actual)
        if head:
            run('git', 'merge-base', '--is-ancestor', head, actual)
        head, already_applied = actual, True
    previous = api(repository_path(f"pulls/{int(data['current_pr'])}")) if data['current_pr'] else None
    if previous and (not eligible_pr({**previous, 'labels': [{'name': 'release'}]})
                     or (previous['head']['sha'] != data['current_head']
                         and not (already_applied and previous['head']['ref'] == branch and previous['head']['sha'] == head))):
        raise ReleaseError('The release PR changed after planning; rerun preparation.')
    opened = [p for p in pages(repository_path(f'pulls?state=open&base={BASE}&per_page=100'))
              if p['head']['ref'] == branch and eligible_pr({**p, 'labels': [{'name': 'release'}]})]
    if len(opened) > 1 or (opened and opened[0]['head']['sha'] != head):
        raise ReleaseError('The prepared branch has a conflicting open PR.')
    current = opened[0] if opened else None
    if previous and previous['state'] != 'open' and not (already_applied and current and previous['number'] != current['number']):
        raise ReleaseError('The reviewed preparation PR was closed; rerun preparation instead of reopening it.')
    push_preparation(branch, head, base, values, f'chore: prepare release {value}')
    body = f"Prepare **{value}** ({intent['channel']}) from `{base}`.\n\nReview Cargo metadata, CHANGELOG.md and the complete preview. Squash merging this PR authorizes publication of this version.\n\nPrevious stable: {predecessor or 'none (first release)'}.\n"
    if current and current['head']['ref'] == branch:
        pr = api(repository_path(f"pulls/{current['number']}"), method='PATCH', payload={'title': f'chore: release {value}', 'body': body})
    else:
        pr = api(repository_path('pulls'), method='POST', payload={'head': branch, 'base': BASE, 'title': f'chore: release {value}', 'body': body})
    if previous and previous['state'] == 'open' and previous['number'] != pr['number']:
        api(repository_path(f"pulls/{previous['number']}"), method='PATCH', payload={'state': 'closed'})
    api(repository_path(f"issues/{pr['number']}/labels"), method='POST', payload={'labels': ['release']})
    print(pr['html_url'])


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('phase', choices=('plan', 'apply'))
    parser.add_argument('--version', default='')
    args = parser.parse_args()
    if os.environ.get('GITHUB_REPOSITORY') != REPO or os.environ.get('GITHUB_REF') != f'refs/heads/{BASE}':
        raise ReleaseError('Preparation is restricted to the configured repository base branch.')
    if args.phase == 'plan':
        plan(version(args.version) if args.version else None)
    else:
        apply()
