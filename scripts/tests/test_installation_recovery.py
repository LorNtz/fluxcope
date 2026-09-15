"""Pin before formula evaluation and bind master verification to release identity."""
import base64
from contextlib import ExitStack
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import homebrew_release as brew
import release_status as status
import release_support as support
import verify_installations as verify


class FormulaTrustTests(unittest.TestCase):
    def setUp(self):
        self.identity = {'version': '0.1.0', 'source': 'a' * 40, 'channel': 'stable'}
        self.commit = 'c' * 40
        self.formula = b'class Fluxcope < Formula\n  version "0.1.0"\nend\n'
        self.target = support.CONFIG['platforms'][0]['target']

    def test_tap_resolution_can_read_existing_history_but_never_write(self):
        for scenario in ('exact', 'historical', 'missing', 'network-error'):
            with self.subTest(scenario=scenario):
                def api(path, **kwargs):
                    self.assertEqual(kwargs.get('method', 'GET'), 'GET')
                    if path == f'repos/{brew.TAP}':
                        return {'private': False, 'default_branch': 'master'}
                    if '/git/ref/' in path:
                        return {'object': {'sha': 'd' * 40}}
                    if '/commits?' in path:
                        return [{'sha': self.commit}]
                    raise AssertionError(path)
                def formula(ref):
                    if scenario == 'network-error':
                        raise support.ReleaseError('Network failed')
                    if scenario == 'missing':
                        return None
                    data = self.formula
                    if scenario == 'historical' and ref != self.commit:
                        data = data.replace(b'0.1.0', b'0.2.0')
                    return {'content': base64.b64encode(data).decode()}
                with patch.object(brew, 'api', side_effect=api), patch.object(brew, 'formula_at', side_effect=formula):
                    if scenario == 'missing':
                        with self.assertRaises(brew.FormulaUnavailable):
                            brew.resolve_tap_commit(self.identity, self.formula, update=False)
                    elif scenario == 'network-error':
                        with self.assertRaises(support.ReleaseError) as error:
                            brew.resolve_tap_commit(self.identity, self.formula, update=False)
                        self.assertNotIsInstance(error.exception, brew.FormulaUnavailable)
                    else:
                        actual = brew.resolve_tap_commit(self.identity, self.formula, update=False)
                        self.assertEqual(actual, self.commit if scenario == 'historical' else 'd' * 40)

    def test_formula_trust_requires_pinned_commit_and_identical_bytes(self):
        for fault in (None, 'origin', 'head', 'formula', 'commit'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                metadata, tap = root / 'metadata', root / 'tap'
                metadata.mkdir()
                (metadata / 'fluxcope.rb').write_bytes(self.formula)
                (metadata / f'{self.target}-smoke.json').write_text(json.dumps({'binary_sha256': 'expected'}))
                calls = []
                def run(*args, **kwargs):
                    calls.append(args)
                    if args[:2] == ('brew', '--repo'):
                        return str(tap)
                    if args[:3] == ('git', 'clone', '--no-checkout'):
                        (tap / 'Formula').mkdir(parents=True)
                    if args[3:6] == ('remote', 'get-url', 'origin'):
                        return 'https://github.com/' + ('attacker/tap' if fault == 'origin' else brew.TAP) + '.git'
                    if args[3:5] == ('checkout', '--detach'):
                        (tap / 'Formula/fluxcope.rb').write_bytes(b'changed' if fault == 'formula' else self.formula)
                    if args[3:5] == ('rev-parse', 'HEAD'):
                        return 'f' * 40 if fault == 'head' else self.commit
                    if args[:2] == ('brew', 'trust'):
                        self.assertEqual(args, ('brew', 'trust', '--formula', f'{brew.TAP_NAME}/fluxcope'))
                        self.assertEqual((tap / 'Formula/fluxcope.rb').read_bytes(), self.formula)
                        self.assertTrue(any(c[3:5] == ('rev-parse', 'HEAD') for c in calls))
                    if args[:2] == ('brew', 'install'):
                        self.assertTrue(any(c[:2] == ('brew', 'trust') for c in calls))
                    if args[:2] == ('brew', '--prefix'):
                        return str(root / 'installed')
                    if args[:2] == ('python3', 'scripts/smoke.py'):
                        (root / 'target').mkdir()
                        (root / f'target/homebrew-{self.target}.json').write_text('{}')
                    return ''
                with patch.dict(os.environ, {'GITHUB_ACTIONS': 'true'}), patch.object(brew, 'ROOT', root), \
                        patch.object(brew, 'run', side_effect=run), patch.object(brew, 'digest', return_value='expected'):
                    if fault:
                        with self.assertRaises(support.ReleaseError):
                            brew.install(self.identity, metadata, '--bad' if fault == 'commit' else self.commit, self.target)
                        self.assertFalse(any(c[:2] in (('brew', 'trust'), ('brew', 'install'), ('brew', 'tap')) for c in calls))
                    else:
                        brew.install(self.identity, metadata, self.commit, self.target)
                        report = json.loads((root / f'target/homebrew-{self.target}.json').read_text())
                        self.assertEqual(report['source'], self.identity['source'])
                        self.assertEqual(report['tap_commit'], self.commit)


class MasterVerificationTests(unittest.TestCase):
    def setUp(self):
        self.identity = {'version': '0.1.0', 'source': 'a' * 40, 'channel': 'stable'}
        self.env = {'GITHUB_REPOSITORY': support.REPO, 'GITHUB_REF': 'refs/heads/master',
                    'GITHUB_EVENT_NAME': 'workflow_dispatch', 'GITHUB_SHA': 'b' * 40}

    def test_master_controller_cannot_change_release_source_or_admit_other_refs(self):
        for fault in (None, 'repository', 'ref', 'event', 'tag', 'rc'):
            with self.subTest(fault=fault), ExitStack() as stack:
                env = dict(self.env)
                if fault in ('repository', 'ref', 'event'):
                    env[{'repository': 'GITHUB_REPOSITORY', 'ref': 'GITHUB_REF', 'event': 'GITHUB_EVENT_NAME'}[fault]] = 'untrusted'
                stack.enter_context(patch.dict(os.environ, env))
                identity = {**self.identity, 'channel': 'rc'} if fault == 'rc' else self.identity
                authorize = stack.enter_context(patch.object(verify, 'authorized_version', return_value=identity))
                stack.enter_context(patch.object(verify, 'resolve_tag', return_value='f' * 40 if fault == 'tag' else self.identity['source']))
                package = stack.enter_context(patch.object(verify, 'source_package', return_value=({'sha256': 'digest'}, b'crate')))
                check = stack.enter_context(patch.object(verify, 'check_registry'))
                if fault:
                    with self.assertRaises(support.ReleaseError):
                        verify.authorize('v0.1.0')
                    package.assert_not_called()
                    if fault in ('repository', 'ref', 'event'):
                        authorize.assert_not_called()
                else:
                    self.assertEqual(verify.authorize('v0.1.0')['source'], self.identity['source'])
                    check.assert_called_once_with(identity, 'digest')

    def test_only_exact_master_verification_runs_are_selected_and_observed(self):
        workflow = 'verify-installations.yml'
        names = {name: index for index, name in enumerate(status.WORKFLOWS, 1)}
        good = {'id': 900, 'workflow_id': names[workflow], 'repository': {'full_name': support.REPO},
                'run_attempt': 1, 'event': 'workflow_dispatch', 'head_branch': 'master',
                'head_sha': 'b' * 40, 'display_title': 'Verify v0.1.0'}
        variants = [good, *[{**good, key: value} for key, value in (
            ('event', 'push'), ('head_branch', 'feature'), ('display_title', 'Verify v0.2.0'),
            ('repository', {'full_name': 'attacker/fork'}), ('workflow_id', -1))]]
        def api(path):
            if '/runs?' in path:
                if f"/{names[workflow]}/" in path:
                    self.assertNotIn('head_sha=', path)
                    return {'workflow_runs': variants}
                return {'workflow_runs': []}
            return {'id': names[path.rsplit('/', 1)[1]]}
        with patch.object(status, 'api', side_effect=api):
            self.assertEqual(status.exact_runs(self.identity), [{**good, 'file': workflow}])
        for run in variants:
            with self.subTest(run=run), patch.object(status, 'api', side_effect=lambda p: run if '/actions/runs/' in p else api(p)), \
                    patch.object(status, 'authorized_version', return_value=self.identity) as authorize:
                result = status.event_identity({'workflow_run': run})
                if run == good or run['display_title'] == 'Verify v0.2.0':
                    self.assertEqual(result, self.identity)
                    authorize.assert_called_once_with(run['display_title'][8:])
                else:
                    self.assertIsNone(result)

    def test_verification_evidence_still_requires_released_source(self):
        run = {'id': 1, 'file': 'verify-installations.yml', 'head_sha': 'b' * 40}
        release = {'assets': [{'name': 'archive', 'digest': 'sha256:abc'}]}
        report = {**self.identity, 'schema': 1, 'result': 'passed',
                  'signer': f'{support.REPO}/.github/workflows/release.yml', 'assets': {'archive': 'sha256:abc'}}
        for source in (self.identity['source'], run['head_sha']):
            with self.subTest(source=source), patch.object(status, 'successful_job', return_value={'evidence_attempt': 1}), \
                    patch.object(status, 'artifact_files', return_value={'public-verification.json': json.dumps({**report, 'source': source}).encode()}):
                if source == self.identity['source']:
                    self.assertTrue(status.public_evidence(self.identity, [run], release))
                else:
                    with self.assertRaises(support.ReleaseError):
                        status.public_evidence(self.identity, [run], release)


if __name__ == '__main__':
    unittest.main()
