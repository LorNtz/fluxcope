import copy
import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest
from unittest.mock import patch

import preview_identity as preview
from release_support import ReleaseError, version

MANIFEST = b'''[package]
name = "fluxcope"
version = "0.1.0"
repository = "https://github.com/LorNtz/fluxcope"
[dependencies]
example = "1"
'''
LOCK = b'''version = 4

[[package]]
name = "example"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "fluxcope"
version = "0.1.0"
dependencies = ["example"]
'''


def current_run():
    return {'id': 123, 'display_title': 'Preview #7 ' + 'a' * 40 + ' macos-arm64',
            'head_sha': 'c' * 40, 'created_at': '2026-09-16T00:00:00Z',
            'actor': {'login': 'LorNtz'}, 'triggering_actor': {'login': 'LorNtz'}, 'run_attempt': 1}


class IdentityTests(unittest.TestCase):
    def setUp(self):
        self.identity = preview.identity_from_run(current_run())

    def test_preview_namespace_never_enters_stable_parser(self):
        self.assertEqual(self.identity['version'], '0.0.0-preview.123')
        with self.assertRaises(ReleaseError):
            version(self.identity['version'])
        for bad in ('01', '-1', '0', '1.2', '../123', '1\n', True):
            with self.subTest(bad=bad), self.assertRaises(ReleaseError):
                preview.preview_id(bad)

    def test_profiles_cannot_select_runners_or_commands(self):
        self.assertEqual(len(preview.platforms('all')), 4)
        self.assertEqual(len(preview.platforms('macos-arm64')), 1)
        for bad in ('macos-15-large', 'self-hosted', '$(whoami)', ''):
            with self.assertRaises(ReleaseError):
                preview.platforms(bad)

    def test_overlay_changes_only_declared_metadata(self):
        result = preview.overlay(MANIFEST, LOCK, self.identity)
        manifest = tomllib.loads(result['Cargo.toml'].decode())
        manifest['package']['version'] = '0.1.0'
        self.assertEqual(manifest, tomllib.loads(MANIFEST.decode()))
        lock = tomllib.loads(result['Cargo.lock'].decode())
        next(p for p in lock['package'] if p['name'] == 'fluxcope')['version'] = '0.1.0'
        self.assertEqual(lock, tomllib.loads(LOCK.decode()))
        dist = tomllib.loads(result['dist-workspace.toml'].decode())['dist']
        self.assertEqual(dist['targets'], self.identity['targets'])
        self.assertEqual(dist['installers'], [])

    def test_workspace_and_dist_overrides_rejected(self):
        for extra in (b'\n[workspace]\nmembers=[]\n', b'\n[package.metadata.dist]\ninstallers=["shell"]\n'):
            with self.assertRaises(ReleaseError):
                preview.overlay(MANIFEST + extra, LOCK, self.identity)
        with self.assertRaises(ReleaseError):
            preview.overlay(MANIFEST, LOCK.replace(b'name = "fluxcope"', b'name = "different"'), self.identity)

    def test_draft_pr_allowed_but_fork_closed_reserved_and_changed_head_rejected(self):
        pr = {'head': {'ref': 'feature/test', 'sha': self.identity['source'], 'repo': {'full_name': preview.REPO}},
              'base': {'ref': 'master', 'repo': {'full_name': preview.REPO}}, 'state': 'open', 'draft': True}
        preview.validate_pr(pr, self.identity['source'], exact=True)
        for kind in ('fork', 'closed', 'reserved', 'head'):
            changed = copy.deepcopy(pr)
            if kind == 'fork':
                changed['head']['repo']['full_name'] = 'someone/fork'
            elif kind == 'closed':
                changed['state'] = 'closed'
            elif kind == 'reserved':
                changed['head']['ref'] = 'rc/0.2.0-rc.1'
            else:
                changed['head']['sha'] = 'b' * 40
            with self.subTest(kind=kind), self.assertRaises(ReleaseError):
                preview.validate_pr(changed, self.identity['source'], exact=True)
        changed['head']['ref'] = 'feature/test'
        preview.validate_pr(changed, self.identity['source'], exact=False)

    def test_real_git_snapshot_is_reproducible_and_leaves_feature_head_unchanged(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            origin = root / 'origin'
            controller = root / 'controller'
            origin.mkdir()
            def git(path, *args):
                return subprocess.check_output(['git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', *args],
                                               cwd=path, stderr=subprocess.DEVNULL).decode().strip()
            git(origin, 'init')
            for name, data in {'Cargo.toml': MANIFEST, 'Cargo.lock': LOCK, 'dist-workspace.toml': b'[dist]\n', 'app.txt': b'unchanged'}.items():
                (origin / name).write_bytes(data)
            git(origin, 'add', '.')
            git(origin, 'commit', '-m', 'fixture')
            source = git(origin, 'rev-parse', 'HEAD')
            git(root, 'clone', str(origin), str(controller))
            identity = {**self.identity, 'source': source}
            original_git = preview.git
            with patch.object(preview, 'git', side_effect=lambda *a, **k: original_git(*a, cwd=controller, **k)):
                first = preview.snapshot(identity)
                second = preview.snapshot(identity)
                self.assertEqual(first, second)
                self.assertEqual(git(controller, 'rev-parse', 'HEAD'), source)
                changed = git(controller, 'diff', '--name-only', source, first['snapshot']).splitlines()
                self.assertEqual(set(changed), {'Cargo.toml', 'Cargo.lock', 'dist-workspace.toml'})
                self.assertEqual(git(controller, 'rev-parse', first['snapshot'] + '^'), source)
                self.assertNotEqual(preview.snapshot({**identity, 'id': 124, 'version': '0.0.0-preview.124'})['snapshot'], first['snapshot'])
                (origin / 'Cargo.lock').unlink()
                (origin / 'Cargo.lock').symlink_to('app.txt')
                git(origin, 'add', '.')
                git(origin, 'commit', '-m', 'symlink')
                with self.assertRaises(ReleaseError):
                    preview.snapshot({**identity, 'source': git(origin, 'rev-parse', 'HEAD')})

    def test_rerun_uses_prior_gate_but_rechecks_actor(self):
        current = current_run()
        current['run_attempt'] = 2
        environment = {'GITHUB_REPOSITORY': preview.REPO, 'GITHUB_REF': 'refs/heads/master',
                       'GITHUB_EVENT_NAME': 'workflow_dispatch', 'GITHUB_RUN_ID': '123', 'GITHUB_SHA': current['head_sha']}
        pr = {'head': {'ref': 'feature/test', 'sha': 'b' * 40, 'repo': {'full_name': preview.REPO}},
              'base': {'ref': 'master', 'repo': {'full_name': preview.REPO}}, 'state': 'open'}
        def api(endpoint):
            if endpoint.endswith('/master'):
                return {'protected': True}
            if endpoint.endswith('/permission'):
                return {'role_name': 'maintain'}
            return pr
        with patch.dict(os.environ, environment), patch.object(preview, 'trusted_run', return_value=current), \
                patch.object(preview, 'api', side_effect=api), patch.object(preview, 'run_jobs', return_value={'Preview gate': {'conclusion': 'success'}}):
            actual, _ = preview.authorize(exact=True)
            self.assertEqual(actual['source'], 'a' * 40)
        with patch.dict(os.environ, environment), patch.object(preview, 'trusted_run', return_value=current), \
                patch.object(preview, 'api', return_value={'protected': True, 'role_name': 'write'}):
            with self.assertRaises(ReleaseError):
                preview.authorize()


if __name__ == '__main__':
    unittest.main()
