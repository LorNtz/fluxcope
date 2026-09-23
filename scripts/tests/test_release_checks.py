"""Release waiting regressions use synthetic GitHub facts; no remote mutations."""
from copy import deepcopy
import io
import json
import unittest
from contextlib import ExitStack, redirect_stdout
from unittest.mock import patch

import release
import release_checks as checks
from release_support import ReleaseError


def pull_request(state='blocked', **values):
    return {'number': 21, 'html_url': 'https://github.com/LorNtz/fluxcope/pull/21',
            'head': {'sha': 'a' * 40}, 'base': {'sha': 'b' * 40, 'ref': 'master'},
            'state': 'open', 'draft': False, 'merged': False, 'mergeable': True,
            'mergeable_state': state, 'title': 'chore: release 0.2.0', 'body': '', **values}


def check(name='Release preview', bucket='pass', state='SUCCESS'):
    return {'name': name, 'bucket': bucket, 'state': state, 'link': 'https://github.com/check/1'}


class RequiredCheckTests(unittest.TestCase):
    def test_combines_rulesets_and_classic_protection(self):
        rules = [{'type': 'pull_request'}, {'type': 'required_status_checks', 'parameters': {
            'required_status_checks': [{'context': 'Release preview', 'integration_id': 15368}]}}]
        protection = {'requiresStatusChecks': True, 'requiredStatusCheckContexts': ['Package', 'Tests (Linux)']}
        data = {'data': {'repository': {'ref': {'branchProtectionRule': protection}}}}
        with patch.object(checks, 'pages', return_value=rules) as pages, \
                patch.object(checks, 'run', return_value=json.dumps(data)) as command:
            self.assertEqual(checks.required_checks('release/main'),
                             {'Release preview', 'Package', 'Tests (Linux)'})
        self.assertIn('release%2Fmain', pages.call_args.args[0])
        self.assertIn('ref=refs/heads/release/main', command.call_args.args)

    def test_absent_or_disabled_classic_protection_adds_no_checks(self):
        for protection in (None, {'requiresStatusChecks': False, 'requiredStatusCheckContexts': ['Old']}):
            data = {'data': {'repository': {'ref': {'branchProtectionRule': protection}}}}
            with self.subTest(protection=protection), patch.object(checks, 'pages', return_value=[]), \
                    patch.object(checks, 'run', return_value=json.dumps(data)):
                self.assertEqual(checks.required_checks('master'), set())

    def test_classic_protection_access_errors_are_not_ignored(self):
        with patch.object(checks, 'pages', return_value=[]), \
                patch.object(checks, 'run', side_effect=ReleaseError('HTTP 403')):
            with self.assertRaisesRegex(ReleaseError, '403'):
                checks.required_checks('master')

    def test_missing_base_branch_is_not_unprotected(self):
        data = {'data': {'repository': {'ref': None}}}
        with patch.object(checks, 'pages', return_value=[]), \
                patch.object(checks, 'run', return_value=json.dumps(data)):
            with self.assertRaisesRegex(ReleaseError, 'base branch master is missing'):
                checks.required_checks('master')

    def test_ruleset_access_errors_are_not_an_empty_required_set(self):
        with patch.object(checks, 'pages', side_effect=ReleaseError('Forbidden (HTTP 403)')):
            with self.assertRaisesRegex(ReleaseError, 'Forbidden'):
                checks.required_checks('master')

    def test_check_states_remain_data(self):
        values = [check(bucket='fail', state='FAILURE'), check('Tests', 'pending', 'QUEUED')]
        with patch.object(checks, 'run', return_value=json.dumps(values)):
            self.assertEqual(checks.reported_checks(21), values)

    def test_no_registered_checks_is_pending_but_transport_errors_fail(self):
        for message in ("no checks reported on the 'branch' branch",
                        "no required checks reported on the 'branch' branch"):
            with patch.object(checks, 'run', side_effect=ReleaseError('gh pr failed (1): ' + message)):
                self.assertEqual(checks.reported_checks(21), [])
        with patch.object(checks, 'run', side_effect=ReleaseError('gh pr failed (1): HTTP 401')):
            with self.assertRaisesRegex(ReleaseError, '401'):
                checks.reported_checks(21)


class MergeWaitTests(unittest.TestCase):
    def setUp(self):
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.elapsed = 0
        self.output = io.StringIO()
        self.stack.enter_context(redirect_stdout(self.output))
        self.stack.enter_context(patch.object(checks.time, 'monotonic', side_effect=lambda: self.elapsed))
        self.sleep = self.stack.enter_context(patch.object(checks.time, 'sleep', side_effect=self.advance))
        self.pr = pull_request()
        self.api = self.stack.enter_context(patch.object(checks, 'api', side_effect=lambda _: deepcopy(self.pr)))
        self.required = self.stack.enter_context(patch.object(checks, 'required_checks', return_value={'Release preview'}))
        self.reported = self.stack.enter_context(patch.object(checks, 'reported_checks', return_value=[check()]))
        self.run = self.stack.enter_context(patch.object(checks, 'run', return_value='{"reviewDecision": null}'))

    def advance(self, seconds):
        self.elapsed += seconds

    def wait(self, **kwargs):
        return checks.wait_for_merge(deepcopy(self.pr), resume='just release', **kwargs)

    def test_late_downstream_job_must_appear_and_finish_before_merge(self):
        self.pr['mergeable_state'] = 'clean'
        self.required.return_value = {'Source quality', 'Release preview'}
        self.reported.side_effect = [
            [check('Source quality')],
            [check('Source quality'), check(bucket='pending', state='QUEUED')],
            [check('Source quality'), check()],
        ]
        result = self.wait()
        self.assertEqual(result['head'], self.pr['head'])
        self.assertEqual(self.sleep.call_count, 2)
        self.assertIn('Release preview (not reported yet)', self.output.getvalue())
        self.assertIn('Release preview (queued)', self.output.getvalue())
        self.run.assert_not_called()

    def test_empty_check_set_never_means_all_required_checks_passed(self):
        self.pr['mergeable_state'] = 'clean'
        self.reported.side_effect = [[], [check()]]
        self.wait()
        self.sleep.assert_called_once()

    def test_rules_added_while_waiting_are_included(self):
        self.pr['mergeable_state'] = 'clean'
        self.required.side_effect = [{'Release preview'}, {'Release preview', 'Package'},
                                     {'Release preview', 'Package'}]
        self.reported.side_effect = [[], [check()], [check(), check('Package')]]
        self.wait()
        self.assertEqual(self.sleep.call_count, 2)
        self.assertIn('Package (not reported yet)', self.output.getvalue())

    def test_missing_checks_are_reported_even_if_other_checks_pass(self):
        self.reported.return_value = []
        with self.assertRaisesRegex(ReleaseError, 'resume with just release'):
            self.wait(timeout=20)
        self.assertEqual(self.elapsed, 20)
        self.assertIn('not reported yet', self.output.getvalue())

    def test_failure_and_cancellation_identify_check_and_link(self):
        for bucket, state in [('fail', 'FAILURE'), ('cancel', 'CANCELLED')]:
            with self.subTest(state=state):
                self.reported.return_value = [check(bucket=bucket, state=state)]
                with self.assertRaisesRegex(ReleaseError, 'Release preview: ' + state) as failure:
                    self.wait()
                self.assertIn('https://github.com/check/1', str(failure.exception))
                self.assertIn('then just release', str(failure.exception))
        self.sleep.assert_not_called()

    def test_skipped_and_neutral_checks_respect_github_mergeability(self):
        self.pr['mergeable_state'] = 'clean'
        self.reported.return_value = [check(bucket='skipping', state='SKIPPED'),
                                      check('Package', 'pass', 'NEUTRAL')]
        self.assertEqual(self.wait()['mergeable_state'], 'clean')

    def test_github_mergeability_is_allowed_to_settle_after_checks_pass(self):
        states = [pull_request(), pull_request(), pull_request('unknown', mergeable=None),
                  pull_request('unknown', mergeable=None), pull_request('clean'), pull_request('clean')]
        self.api.side_effect = states
        self.assertEqual(self.wait()['mergeable_state'], 'clean')
        self.assertEqual(self.sleep.call_count, 2)
        self.assertIn('waiting for GitHub merge status', self.output.getvalue())

    def test_persistent_block_after_successful_checks_gets_specific_guidance(self):
        with self.assertRaisesRegex(ReleaseError, 'All required checks passed.*as blocked') as failure:
            self.wait()
        self.assertEqual(self.elapsed, 120)
        self.assertIn('outstanding conversations and branch rules', str(failure.exception))

    def test_conflicts_and_outdated_base_do_not_wait_for_checks(self):
        for state, text in [('dirty', 'merge conflicts'), ('behind', 'updated with master')]:
            with self.subTest(state=state):
                self.pr['mergeable_state'] = state
                with self.assertRaisesRegex(ReleaseError, text):
                    self.wait()
        self.sleep.assert_not_called()

    def test_required_review_is_actionable(self):
        for decision in ('REVIEW_REQUIRED', 'CHANGES_REQUESTED'):
            with self.subTest(decision=decision):
                self.run.return_value = json.dumps({'reviewDecision': decision})
                with self.assertRaisesRegex(ReleaseError, 'needs review approval'):
                    self.wait()
        self.sleep.assert_not_called()

    def test_revision_change_during_check_requests_invalidates_result(self):
        for changed in ('head', 'base'):
            with self.subTest(changed=changed):
                newer = deepcopy(self.pr)
                newer[changed]['sha'] = 'c' * 40
                self.api.side_effect = [deepcopy(self.pr), newer]
                self.reported.return_value = [check(bucket='fail', state='FAILURE')]
                self.assertEqual(self.wait(), newer)
        self.sleep.assert_not_called()

    def test_closed_draft_and_already_merged_prs(self):
        self.pr['state'] = 'closed'
        with self.assertRaisesRegex(ReleaseError, 'closed without merging'):
            self.wait()
        self.pr.update(state='open', draft=True)
        with self.assertRaisesRegex(ReleaseError, 'gh pr ready 21'):
            self.wait()
        self.pr.update(state='closed', merged=True)
        self.assertTrue(self.wait()['merged'])
        self.reported.assert_not_called()

    def test_interrupt_keeps_stage_specific_resume_command(self):
        self.reported.return_value = []
        self.sleep.side_effect = KeyboardInterrupt
        for resume in ('just ship', 'just release'):
            with self.subTest(resume=resume), self.assertRaises(KeyboardInterrupt) as stopped:
                checks.wait_for_merge(self.pr, resume=resume)
            self.assertIn('resume with ' + resume, str(stopped.exception))

    def test_network_errors_are_not_reinterpreted_as_pending(self):
        self.reported.side_effect = ReleaseError('HTTP 403 rate limit')
        with self.assertRaisesRegex(ReleaseError, '403'):
            self.wait()
        self.sleep.assert_not_called()


class ReviewIntegrationTests(unittest.TestCase):
    def test_shared_feature_and_release_paths_wait_before_approval_and_merge(self):
        for is_release in (False, True):
            with self.subTest(release=is_release), ExitStack() as stack:
                original = pull_request('clean')
                merged = {**original, 'merged': True, 'merge_commit_sha': 'd' * 40}
                stack.enter_context(redirect_stdout(io.StringIO()))
                stack.enter_context(patch.object(release, 'api', side_effect=[original, original, merged]))
                stack.enter_context(patch.object(release, 'pr_intent', return_value={'version': '0.2.0', 'channel': 'stable'}))
                order = []
                waiter = stack.enter_context(patch.object(release, 'wait_for_merge',
                    side_effect=lambda *a, **kw: order.append('wait') or original))
                stack.enter_context(patch.object(release, 'confirm', side_effect=lambda _: order.append('confirm')))
                command = stack.enter_context(patch.object(release, 'run', side_effect=lambda *a, **kw: order.append('merge')))
                self.assertEqual(release.review_and_merge(original, release=is_release), merged)
                waiter.assert_called_once_with(original, resume='just release' if is_release else 'just ship')
                self.assertEqual(order, ['wait', 'confirm', 'merge'])
                self.assertIn('--match-head-commit', command.call_args.args)
                self.assertIn(original['head']['sha'], command.call_args.args)

    def test_changed_head_and_base_are_reviewed_again_before_confirmation(self):
        for field in ('head', 'base'):
            with self.subTest(field=field), ExitStack() as stack:
                original = pull_request('clean')
                updated = deepcopy(original)
                updated[field]['sha'] = 'c' * 40
                merged = {**updated, 'merged': True, 'merge_commit_sha': 'd' * 40}
                output = stack.enter_context(redirect_stdout(io.StringIO()))
                stack.enter_context(patch.object(release, 'api', side_effect=[original, updated, updated, merged]))
                intent = stack.enter_context(patch.object(release, 'pr_intent', return_value={'version': '0.2.0', 'channel': 'stable'}))
                waiter = stack.enter_context(patch.object(release, 'wait_for_merge', side_effect=[updated, updated]))
                approval = stack.enter_context(patch.object(release, 'confirm'))
                command = stack.enter_context(patch.object(release, 'run'))
                release.review_and_merge(original, release=True)
                self.assertEqual(waiter.call_count, 2)
                self.assertEqual(intent.call_count, 2)
                self.assertIn('head or base changed', output.getvalue())
                approval.assert_called_once()
                self.assertIn(updated['head']['sha'], command.call_args.args)

    def test_base_change_during_confirmation_requires_new_review_and_approval(self):
        original = pull_request('clean')
        updated = deepcopy(original)
        updated['base']['sha'] = 'c' * 40
        merged = {**updated, 'merged': True, 'merge_commit_sha': 'd' * 40}
        with ExitStack() as stack:
            output = stack.enter_context(redirect_stdout(io.StringIO()))
            stack.enter_context(patch.object(release, 'api', side_effect=[original, updated, updated, updated, merged]))
            stack.enter_context(patch.object(release, 'pr_intent', return_value={'version': '0.2.0', 'channel': 'stable'}))
            waiter = stack.enter_context(patch.object(release, 'wait_for_merge', side_effect=[original, updated]))
            approval = stack.enter_context(patch.object(release, 'confirm'))
            command = stack.enter_context(patch.object(release, 'run'))
            release.review_and_merge(original, release=True)
        self.assertEqual(waiter.call_count, 2)
        self.assertEqual(approval.call_count, 2)
        command.assert_called_once()
        self.assertIn('changed during confirmation', output.getvalue())

    def test_wait_failure_never_reaches_approval_or_merge(self):
        with patch.object(release, 'api', return_value=pull_request()), \
                patch.object(release, 'wait_for_merge', side_effect=ReleaseError('checks failed')), \
                patch.object(release, 'confirm') as approval, patch.object(release, 'run') as command, \
                redirect_stdout(io.StringIO()):
            with self.assertRaisesRegex(ReleaseError, 'checks failed'):
                release.review_and_merge(pull_request(), release=False)
        approval.assert_not_called()
        command.assert_not_called()


if __name__ == '__main__':
    unittest.main()
