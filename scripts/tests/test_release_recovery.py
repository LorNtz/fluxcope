"""Publication failure scenarios and permission boundaries; no real remote writes."""
from contextlib import ExitStack
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import installer_check
import release_gate as gate
import release_recovery as recovery
import release_source as source
import release_status as status
import release_support as support


class ExactSourceTests(unittest.TestCase):
    def setUp(self):
        self.source = 'a' * 40
        self.head = 'b' * 40
        self.intent = {'schema': 1, 'version': '0.1.0', 'channel': 'stable', 'mode': 'explicit', 'previous_stable': None}
        self.pr = {'number': 7, 'merged_at': '2026-09-14T00:00:00Z', 'merge_commit_sha': self.source,
                   'html_url': 'https://github.com/test/pr/7', 'labels': [{'name': 'release'}],
                   'user': {'login': support.CONFIG['release_authors'][0]},
                   'base': {'ref': support.BASE, 'repo': {'full_name': support.REPO}},
                   'head': {'ref': 'release-plz-0.1.0', 'sha': self.head, 'repo': {'full_name': support.REPO}}}
        self.parents = [{'sha': 'c' * 40}]
        self.ancestry = 'diverged'

    def api(self, endpoint):
        if endpoint.endswith(f'commits/{self.source}/pulls?per_page=100'):
            return [self.pr]
        if endpoint.endswith(f'git/commits/{self.source}'):
            return {'parents': self.parents}
        if endpoint.endswith(f'compare/{self.head}...{self.source}'):
            return {'status': self.ancestry}
        if endpoint.endswith(f'compare/{self.source}...{support.BASE}'):
            return {'status': 'ahead'}  # A newer master must not change source selection.
        raise AssertionError(endpoint)

    def file(self, name, ref):
        if name == '.github/release-intent.json':
            return json.dumps(self.intent).encode()
        if name == 'Cargo.toml':
            return b'[package]\nname="fluxcope"\nversion="0.1.0"\n'
        if name == 'Cargo.lock':
            return b'[[package]]\nname="fluxcope"\nversion="0.1.0"\n'
        raise AssertionError(name)

    def invoke(self):
        with patch.object(gate, 'api', side_effect=self.api), patch.object(gate, 'file_at', side_effect=self.file), patch.object(support, 'file_at', side_effect=self.file):
            return gate.merged_release(self.source)

    def test_later_master_push_does_not_change_authorized_source(self):
        self.assertEqual(self.invoke()['source'], self.source)

    def test_merge_commit_and_native_publisher_checkout_are_rejected(self):
        self.parents.append({'sha': self.head})
        with self.assertRaises(support.ReleaseError):
            self.invoke()
        self.parents.pop()
        self.ancestry = 'ahead'
        with self.assertRaises(support.ReleaseError):
            self.invoke()

    def test_unauthorized_actor_cannot_publish_an_otherwise_correct_version(self):
        self.pr['user']['login'] = 'unknown'
        with self.assertRaises(support.ReleaseError):
            self.invoke()


class TagAndRecoveryTests(unittest.TestCase):
    def setUp(self):
        self.identity = {'source': 'a' * 40, 'version': '0.1.0', 'channel': 'stable', 'pr': 7}
        self.package = {'sha256': 'b' * 64}

    def plan(self, *, crate=True, tag=None, release=None, active=None, bootstrap=False, tap=True, public=True):
        with ExitStack() as stack:
            stack.enter_context(patch.object(recovery, 'exact_runs', return_value=active or []))
            stack.enter_context(patch.object(recovery, 'source_package', return_value=(self.package, b'crate')))
            stack.enter_context(patch.object(recovery, 'registry_version', return_value={'checksum': self.package['sha256']} if crate else None))
            stack.enter_context(patch.object(recovery, 'resolve_tag', return_value=tag))
            stack.enter_context(patch.object(recovery, 'optional_api', return_value=release))
            stack.enter_context(patch.object(recovery, 'snapshot', return_value={'state': 'partial', 'public_verified': public}))
            stack.enter_context(patch.object(recovery, 'public_metadata', side_effect=lambda i, d, t: (d / 'fluxcope.rb').write_bytes(b'formula')))
            stack.enter_context(patch.object(recovery, 'resolve_tap_commit', return_value='c' * 40,
                                            side_effect=None if tap else recovery.FormulaUnavailable('missing')))
            return recovery.plan(self.identity, bootstrap=bootstrap)

    def test_crate_success_missing_tag_does_not_retry_registry_upload(self):
        selected = self.plan()
        self.assertEqual(selected['operation'], 'tag-and-distribute')
        with patch.object(recovery, 'ensure_tag') as tag, patch.object(recovery, 'dispatch_distribution') as dispatch, patch.object(recovery, 'api') as write:
            recovery.execute(self.identity, selected['operation'], selected)
            tag.assert_called_once()
            dispatch.assert_called_once()
            write.assert_not_called()

    def test_existing_public_release_repairs_installations_without_reuploading_assets(self):
        selected = self.plan(tag=self.identity['source'], release={'draft': False, 'immutable': True})
        self.assertEqual(selected['operation'], 'verify-installations')
        with patch.object(recovery, 'dispatch') as dispatch, patch.object(recovery, 'dispatch_distribution') as distribution:
            recovery.execute(self.identity, selected['operation'], selected)
            dispatch.assert_called_once_with('verify-installations.yml', support.BASE, {'tag': 'v0.1.0'})
            distribution.assert_not_called()

    def test_expired_public_report_uses_master_verifier_when_formula_exists(self):
        selected = self.plan(tag=self.identity['source'], release={'draft': False, 'immutable': True}, public=False)
        self.assertEqual(selected['operation'], 'verify-installations')
        selected = self.plan(tag=self.identity['source'], release={'draft': False, 'immutable': True}, public=False, tap=False)
        self.assertEqual(selected['operation'], 'verify-public')

    def test_missing_tap_formula_retains_authorized_publication_recovery(self):
        selected = self.plan(tag=self.identity['source'], release={'draft': False, 'immutable': True}, tap=False)
        self.assertEqual(selected['operation'], 'stable-installations')
        with patch.object(recovery, 'dispatch') as dispatch:
            recovery.execute(self.identity, selected['operation'], selected)
        dispatch.assert_called_once_with('publish-homebrew.yml', 'v0.1.0', {'tag': 'v0.1.0'})

    def test_active_run_and_first_upload_do_not_create_duplicate_publications(self):
        self.assertEqual(self.plan(active=[{'id': 123, 'status': 'in_progress', 'html_url': 'run'}])['operation'], 'wait')
        self.assertEqual(self.plan(crate=False, bootstrap=True)['operation'], 'bootstrap-upload')

    def test_stale_recovery_plan_has_no_remote_side_effect(self):
        with patch.object(recovery, 'api') as api, patch.object(recovery, 'dispatch') as dispatch, self.assertRaises(support.ReleaseError):
            recovery.execute(self.identity, 'distribute', {'operation': 'stable-installations'})
        api.assert_not_called()
        dispatch.assert_not_called()

    def test_existing_tag_is_idempotent_and_conflicting_tag_is_never_moved(self):
        with patch.object(source, 'resolve_tag', return_value=self.identity['source']), patch.object(source, 'api') as api:
            source.ensure_tag(self.identity)
            api.assert_not_called()
        with patch.object(source, 'resolve_tag', return_value='f' * 40), patch.object(source, 'api') as api, self.assertRaises(support.ReleaseError):
            source.ensure_tag(self.identity)
        api.assert_not_called()


class ResultAndInstallerTests(unittest.TestCase):
    def test_required_channel_failures_remain_incomplete_and_rc_channels_are_inapplicable(self):
        base = dict(stable=True, crate=True, github=True, tap=True, installed=True, running=False, bootstrap=False)
        self.assertEqual(status.classify(**base), 'complete')
        for missing in ('github', 'tap', 'installed'):
            self.assertEqual(status.classify(**{**base, missing: False}), 'partial')
        self.assertEqual(status.classify(**{**base, 'stable': False, 'crate': False, 'tap': False, 'installed': False}), 'rc-complete')
        self.assertEqual(status.classify(**{**base, 'crate': False, 'github': False, 'bootstrap': True}), 'bootstrap-required')

    def test_failed_job_rerun_preserves_prior_successful_job_attempt(self):
        support.run_jobs.cache_clear()
        source_sha = 'a' * 40
        records = {
            2: [{'name': 'Tests (Linux)', 'head_sha': source_sha, 'conclusion': 'success'}],
            1: [{'name': 'Tests (Linux)', 'head_sha': source_sha, 'conclusion': 'failure'},
                {'name': 'Package', 'head_sha': source_sha, 'conclusion': 'success'}],
        }
        def api(path):
            attempt = int(path.split('/attempts/')[1].split('/')[0])
            return {'jobs': records[attempt]}
        with patch.object(support, 'api', side_effect=api):
            jobs = support.run_jobs(123, 2)
        self.assertEqual(jobs['Package']['evidence_attempt'], 1)
        self.assertEqual(jobs['Tests (Linux)']['evidence_attempt'], 2)
        self.assertEqual(jobs['Tests (Linux)']['conclusion'], 'success')
        support.run_jobs.cache_clear()

    def test_carried_jobs_keep_original_artifacts_despite_new_ids_and_attempts(self):
        original = {'id': 101, 'name': 'Package', 'head_sha': 'a' * 40,
                    'run_attempt': 1, 'status': 'completed', 'conclusion': 'success',
                    'started_at': '2026-09-20T04:48:01Z', 'completed_at': '2026-09-20T05:00:11Z'}
        records = {number: [{**original, 'id': 100 + number, 'run_attempt': number}]
                   for number in (1, 2, 3)}
        support.run_jobs.cache_clear()
        self.addCleanup(support.run_jobs.cache_clear)
        def api(path):
            number = int(path.split('/attempts/')[1].split('/')[0])
            return {'jobs': records[number]}
        with patch.object(support, 'api', side_effect=api):
            job = support.run_jobs(123, 3)['Package']
        self.assertEqual(job['id'], 103)
        self.assertEqual(job['evidence_attempt'], 1)

    def test_actual_rerun_never_falls_back_to_prior_successful_evidence(self):
        original = {'name': 'Package', 'head_sha': 'a' * 40,
                    'status': 'completed', 'conclusion': 'success',
                    'started_at': '2026-09-20T04:48:01Z', 'completed_at': '2026-09-20T05:00:11Z'}
        for conclusion in ('success', 'failure'):
            with self.subTest(conclusion=conclusion):
                newer = {**original, 'conclusion': conclusion,
                         'started_at': '2026-09-20T05:08:01Z', 'completed_at': '2026-09-20T05:10:11Z'}
                records = {3: [newer], 2: [newer], 1: [original]}
                support.run_jobs.cache_clear()
                self.addCleanup(support.run_jobs.cache_clear)
                def api(path):
                    number = int(path.split('/attempts/')[1].split('/')[0])
                    return {'jobs': records[number]}
                with patch.object(support, 'api', side_effect=api):
                    job = support.run_jobs(123, 3)['Package']
                self.assertEqual(job['evidence_attempt'], 2)
                self.assertEqual(job['conclusion'], conclusion)

    def test_normal_source_ci_wait_is_a_running_release(self):
        identity = {'source': 'a' * 40, 'version': '0.1.0', 'channel': 'stable'}
        active = {'id': 123, 'run_attempt': 1, 'file': 'release-plz.yml', 'html_url': 'run', 'status': 'in_progress', 'conclusion': None}
        with patch.object(status, 'exact_runs', return_value=[active]), patch.object(status, 'source_package', side_effect=gate.SourceChecksPending('pending')):
            self.assertEqual(status.snapshot(identity)['state'], 'running')

    def test_untrusted_workflow_run_is_not_used_as_release_evidence(self):
        run = {'id': 1, 'workflow_id': 999, 'repository': {'full_name': 'attacker/fork'}, 'run_attempt': 1, 'event': 'workflow_dispatch'}
        with patch.object(status, 'api', side_effect=lambda p: run if p.endswith('actions/runs/1') else {'id': 2}):
            self.assertIsNone(status.event_identity({'workflow_run': run}))

    def test_missing_digest_tools_fail_closed(self):
        guard = (support.ROOT / 'scripts/installer_guard.sh').read_text()
        result = subprocess.run(['/bin/sh', '-c', guard + '\necho installed'], env={'PATH': '/nonexistent'}, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('installed', result.stdout)

    @unittest.skipUnless(Path('/usr/bin/shasum').exists(), 'macOS shasum fallback fixture')
    def test_shasum_fallback_computes_the_same_digest(self):
        guard = (support.ROOT / 'scripts/installer_guard.sh').read_text()
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / 'shasum').symlink_to('/usr/bin/shasum')
            payload = directory / 'payload'
            payload.write_bytes(b'Fluxcope installer fixture')
            result = subprocess.run(['/bin/sh', '-c', guard + '\nsha256sum -b "$1"', 'fixture', str(payload)],
                                    env={'PATH': str(directory)}, capture_output=True, text=True, check=True)
            self.assertEqual(result.stdout.split()[0], hashlib.sha256(payload.read_bytes()).hexdigest())


if __name__ == '__main__':
    unittest.main()
