"""Exercise signed archival, cancelled observers and recovery without remote writes."""
from contextlib import ExitStack
from copy import deepcopy
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import release as client
import release_replacement as replacement
import release_status as status
import release_support as support
import result_archive as archive


class SignedResultTests(unittest.TestCase):
    def setUp(self):
        self.identity = {'source': 'a' * 40, 'version': '0.1.0-rc.1', 'channel': 'rc', 'pr': 7, 'url': 'pr'}
        self.package = {'sha256': 'b' * 64}
        self.release = {'id': 1, 'draft': False, 'immutable': True, 'assets': [{'name': 'archive', 'digest': 'sha256:' + 'c' * 64}]}
        self.report = {**self.identity, 'schema': 1, 'state': 'rc-complete', 'package': self.package,
                       'public_verified': True, 'homebrew': 'not-applicable', 'registry_install': 'not-applicable', 'observer_run': '123'}
        self.saved = archive.prepare(self.identity, self.report, self.release)
        self.signed = archive.encode(self.saved)

    def verify_signature(self, *args):
        self.assertEqual(args[:3], ('gh', 'attestation', 'verify'))
        self.assertIn(f'{support.REPO}/.github/workflows/release-status.yml', args)
        self.assertIn('refs/heads/master', args)
        self.assertIn('--deny-self-hosted-runners', args)
        if Path(args[3]).read_bytes() != self.signed:
            raise support.ReleaseError('No attestation for these exact bytes')
        return ''

    def with_notes(self, saved):
        return {**self.release, 'body': archive.START + '\n```json\n' + json.dumps(saved, indent=2) + '\n```\n' + archive.END}

    def test_exact_signed_archive_survives_raw_artifact_expiry(self):
        with patch.object(archive, 'run', side_effect=self.verify_signature):
            self.assertEqual(archive.read(self.identity, self.with_notes(self.saved), self.package), self.saved)

    def test_unsigned_or_changed_notes_never_become_success_evidence(self):
        with patch.object(archive, 'run', side_effect=support.ReleaseError('Missing attestation')), self.assertRaises(support.ReleaseError):
            archive.read(self.identity, self.with_notes(self.saved), self.package)
        changed = {**self.saved, 'observer_run': '456'}
        with patch.object(archive, 'run', side_effect=self.verify_signature), self.assertRaises(support.ReleaseError):
            archive.read(self.identity, self.with_notes(changed), self.package)
        for field, value in [('source', 'd' * 40), ('version', '0.2.0-rc.1'), ('public_assets', {})]:
            with self.subTest(field=field), patch.object(archive, 'run') as verifier, self.assertRaises(support.ReleaseError):
                archive.read(self.identity, self.with_notes({**self.saved, field: value}), self.package)
            verifier.assert_not_called()

    def test_non_object_archives_are_rejected_before_verification(self):
        for malformed in (None, [], {**self.saved, 'package': []}, {**self.saved, 'package': None}):
            with self.subTest(malformed=malformed), patch.object(archive, 'run') as verifier, self.assertRaises(support.ReleaseError):
                archive.read(self.identity, self.with_notes(malformed), self.package)
            verifier.assert_not_called()

    def test_damaged_markers_never_write_a_second_broken_block(self):
        for body in (archive.START, archive.END, archive.END + archive.START, archive.START * 2 + archive.END * 2):
            with self.subTest(body=body), patch.object(archive, 'api', return_value={**self.release, 'body': body}) as api, \
                    patch.object(archive, 'run', side_effect=self.verify_signature), self.assertRaises(support.ReleaseError):
                archive.write(self.identity, self.saved)
            self.assertTrue(all(call.kwargs.get('method', 'GET') == 'GET' for call in api.call_args_list))

    def test_bad_archive_needs_independent_live_evidence_before_resigning(self):
        release = {**self.with_notes(self.saved), 'prerelease': True, 'html_url': 'release'}
        for live in (False, True):
            with self.subTest(live=live), ExitStack() as stack:
                stack.enter_context(patch.object(status, 'exact_runs', return_value=[]))
                stack.enter_context(patch.object(status, 'source_package', return_value=(self.package, b'crate')))
                stack.enter_context(patch.object(status, 'resolve_tag', return_value=self.identity['source']))
                stack.enter_context(patch.object(status, 'optional_api', return_value=release))
                stack.enter_context(patch.object(status, 'expected_assets', return_value={'archive'}))
                stack.enter_context(patch.object(status, 'public_evidence', return_value=live))
                stack.enter_context(patch.object(archive, 'read', side_effect=support.ReleaseError('Invalid signature')))
                report = status.snapshot(self.identity)
                self.assertEqual(report['state'], 'rc-complete' if live else 'partial')
                self.assertFalse(report['archived'])
                self.assertEqual(report['public_verified'], live)
                if live:
                    archive.prepare(self.identity, {**report, 'observer_run': '123'}, release)
                else:
                    with self.assertRaises(support.ReleaseError):
                        archive.prepare(self.identity, {**report, 'observer_run': '123'}, release)

    def test_lost_patch_response_is_reconciled_with_signed_remote_facts(self):
        def api(path, method='GET', payload=None):
            if method == 'PATCH':
                raise support.ReleaseError('Connection lost after accepted write')
            return self.release if '/tags/' in path else self.with_notes(self.saved)
        with patch.object(archive, 'api', side_effect=api), patch.object(archive, 'run', side_effect=self.verify_signature):
            archive.write(self.identity, self.saved)

    def test_archive_pending_check_is_actionable_if_later_job_never_starts(self):
        with tempfile.TemporaryDirectory() as temporary, patch.object(status, 'ROOT', Path(temporary)), \
                patch.dict(os.environ, {'GITHUB_RUN_ID': '123', 'GITHUB_STEP_SUMMARY': temporary + '/summary'}), \
                patch.object(status, 'api', side_effect=[{'check_runs': []}, {}]) as api:
            status.write_check(self.identity, {**self.report, 'state': 'partial', 'archive_pending': True})
        payload = api.call_args.kwargs['payload']
        self.assertEqual(payload['status'], 'completed')
        self.assertEqual(payload['conclusion'], 'action_required')

    def test_client_waits_for_active_archiver_then_reports_cancelled_job(self):
        check = {'status': 'completed', 'conclusion': 'action_required', 'output': {'summary': 'Archive pending'},
                 'details_url': f'https://github.com/{support.REPO}/actions/runs/123'}
        with patch.object(client, 'reports', return_value=[self.identity]), patch.object(client, 'read_check', return_value=check), \
                patch.object(client, 'api', side_effect=[{'status': 'in_progress'}, {'status': 'completed', 'conclusion': 'cancelled'}]), \
                patch.object(client.time, 'sleep') as sleep, patch('builtins.print'), self.assertRaises(support.ReleaseError):
            client.status(self.identity['version'])
        sleep.assert_called_once_with(10)


class ReplacementTests(unittest.TestCase):
    def test_classification_run_does_not_trigger_release_observation(self):
        names = {name: index + 1 for index, name in enumerate(status.WORKFLOWS)}
        run = {'id': 5, 'workflow_id': names['release-recovery.yml'], 'repository': {'full_name': support.REPO},
               'run_attempt': 1, 'event': 'workflow_dispatch', 'head_branch': support.BASE, 'display_title': 'Classify 0.1.0'}
        def api(path):
            if path.endswith('actions/runs/5'):
                return run
            return {'id': names[path.rsplit('/', 1)[1]]}
        with patch.object(status, 'api', side_effect=api), patch.object(status, 'authorized_version') as authorize:
            self.assertIsNone(status.event_identity({'workflow_run': run}))
        authorize.assert_not_called()

    def test_other_active_publication_still_blocks_replacement(self):
        identity = {'source': 'a' * 40, 'version': '0.1.0'}
        with patch.object(status, 'exact_runs', return_value=[{'id': 123, 'status': 'in_progress'}]), \
                patch.dict(os.environ, {'GITHUB_RUN_ID': '456'}), patch.object(replacement, 'api') as api, \
                self.assertRaises(support.ReleaseError):
            replacement.record(identity, 'Reviewed replacement is required')
        api.assert_not_called()


if __name__ == '__main__':
    unittest.main()
