"""Real temporary Git refs exercise idempotency and base refresh without network."""
import os
import base64
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import release_prepare as prepare
import release_support as support


class PreparationGitTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='fluxcope-prepare-test-')
        self.root = Path(self.temporary.name)
        self.repo = self.root / 'repo'
        self.repo.mkdir()
        self.remote = self.root / 'remote.git'
        self.environment = {**os.environ, 'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': '/dev/null',
                            'GIT_AUTHOR_NAME': 'Release fixture', 'GIT_COMMITTER_NAME': 'Release fixture',
                            'GIT_AUTHOR_EMAIL': 'fixture@example.com', 'GIT_COMMITTER_EMAIL': 'fixture@example.com',
                            'GIT_TOKEN': 'fixture-not-a-credential'}
        self.git('init', '--initial-branch=master')
        self.git('init', '--bare', str(self.remote))
        self.git('remote', 'add', 'origin', str(self.remote))
        (self.repo / '.github').mkdir()
        (self.repo / 'source.txt').write_text('base implementation')
        for name in prepare.FILES:
            (self.repo / name).write_text('initial metadata')
        self.git('add', '--all')
        self.git('commit', '-m', 'initial fixture')
        self.base = self.git('rev-parse', 'HEAD')
        self.git('push', 'origin', 'master')
        self.values = {name: b'prepared release metadata' for name in prepare.FILES}

    def tearDown(self):
        self.temporary.cleanup()

    def git(self, *args, env=None):
        result = subprocess.run(['git', *args], cwd=self.repo, env=env or self.environment, capture_output=True, text=True)
        if result.returncode:
            raise support.ReleaseError(result.stderr)
        return result.stdout.strip()

    def execute_git(self, *args, env=None, capture=True):
        if args[0] != 'git':
            raise AssertionError(args)
        return self.git(*args[1:], env=env)

    def push(self, head=None, base=None):
        with patch.object(prepare, 'ROOT', self.repo), patch.object(prepare, 'run', side_effect=self.execute_git), \
                patch.object(prepare, 'api', return_value={'status': 'diverged'}), patch.dict(os.environ, self.environment):
            prepare.push_preparation('release-plz-0.1.0', head, base or self.base, self.values, 'chore: release fixture')
        return self.git('--git-dir=' + str(self.remote), 'rev-parse', 'refs/heads/release-plz-0.1.0')

    def test_first_push_and_unchanged_retry_keep_the_same_remote_head(self):
        first = self.push()
        self.assertEqual(self.push(first), first)
        self.assertEqual(self.git('show', first + ':source.txt'), 'base implementation')

    def test_refresh_keeps_new_base_code_and_previous_pr_history(self):
        first = self.push()
        self.git('reset', '--hard', self.base)
        (self.repo / 'source.txt').write_text('new master implementation')
        self.git('add', 'source.txt')
        self.git('commit', '-m', 'feature on master')
        newer = self.git('rev-parse', 'HEAD')
        refreshed = self.push(first, newer)
        self.assertNotEqual(first, refreshed)
        self.git('merge-base', '--is-ancestor', first, refreshed)
        self.git('merge-base', '--is-ancestor', newer, refreshed)
        self.assertEqual(self.git('show', refreshed + ':source.txt'), 'new master implementation')

    def test_stale_lease_cannot_overwrite_another_writer(self):
        first = self.push()
        self.values['CHANGELOG.md'] = b'concurrent reviewed changelog'
        second = self.push(first)
        self.values['CHANGELOG.md'] = b'outdated competing update'
        with self.assertRaises(support.ReleaseError):
            self.push(first)
        actual = self.git('--git-dir=' + str(self.remote), 'rev-parse', 'refs/heads/release-plz-0.1.0')
        self.assertEqual(actual, second)

    def retry_fixed_plan(self, failure, *, existing=False):
        intent = {'schema': 1, 'version': '0.1.0', 'channel': 'stable', 'mode': 'explicit', 'previous_stable': None}
        self.values = {'Cargo.toml': b'[package]\nname="fluxcope"\nversion="0.1.0"\n',
                       'Cargo.lock': b'[[package]]\nname="fluxcope"\nversion="0.1.0"\n',
                       'CHANGELOG.md': b'Initial release notes', '.github/release-intent.json': json.dumps(intent).encode()}
        initial_head = self.push() if existing else None
        self.git('reset', '--hard', self.base)
        self.values['CHANGELOG.md'] = b'Final release notes'
        plan = {'schema': 1, 'base': self.base, 'head': initial_head, 'version': '0.1.0',
                'current_pr': 7 if existing else None, 'current_head': initial_head,
                'files': {name: base64.b64encode(value).decode() for name, value in self.values.items()}}
        (self.repo / 'target').mkdir()
        (self.repo / 'target/release-preparation.json').write_text(json.dumps(plan))
        exists = existing
        failed = False
        writes = []

        def remote_head():
            result = self.git('ls-remote', 'origin', 'refs/heads/release-plz-0.1.0')
            return result.split()[0] if result else None

        def pr():
            return {'number': 7, 'state': 'open', 'html_url': 'fixture PR', 'labels': [],
                    'user': {'login': support.CONFIG['release_authors'][0]},
                    'base': {'ref': support.BASE, 'repo': {'full_name': support.REPO}},
                    'head': {'ref': 'release-plz-0.1.0', 'sha': remote_head(), 'repo': {'full_name': support.REPO}}}

        def api(path, method='GET', payload=None):
            nonlocal exists, failed
            if method == 'GET':
                if path.endswith('git/ref/heads/master'):
                    return {'object': {'sha': self.base}}
                if '/compare/' in path:
                    return {'status': 'diverged'}
                if path.endswith('pulls/7'):
                    return pr()
                raise AssertionError(path)
            writes.append((method, path))
            if path.endswith('/pulls'):
                exists = True
            stage = 'labels' if path.endswith('/labels') else ('create' if method == 'POST' else 'update')
            if stage == failure and not failed:
                failed = True
                raise support.ReleaseError('Simulated lost API response after accepted write')
            return pr()

        def optional(path):
            head = remote_head()
            return {'object': {'sha': head}} if head else None

        with patch.object(prepare, 'ROOT', self.repo), patch.object(prepare, 'run', side_effect=self.execute_git), \
                patch.object(prepare, 'api', side_effect=api), patch.object(prepare, 'optional_api', side_effect=optional), \
                patch.object(prepare, 'pages', side_effect=lambda path: [pr()] if exists else []), patch.dict(os.environ, self.environment):
            with self.assertRaises(support.ReleaseError):
                prepare.apply()
            written_head = remote_head()
            prepare.apply()  # The exact same plan, not a recomputed head/lease.
        self.assertEqual(remote_head(), written_head)
        self.assertTrue(writes[-1][1].endswith('/labels'))
        self.assertEqual(sum(path.endswith('/pulls') for _, path in writes), 0 if existing else 1)

    def test_fixed_plan_retry_after_created_pr_response_is_lost(self):
        self.retry_fixed_plan('create')

    def test_fixed_plan_retry_after_label_failure(self):
        self.retry_fixed_plan('labels')

    def test_fixed_plan_retry_after_existing_pr_update_failure(self):
        self.retry_fixed_plan('update', existing=True)


if __name__ == '__main__':
    unittest.main()
