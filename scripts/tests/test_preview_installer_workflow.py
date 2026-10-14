"""Check the installer trust boundary across CI jobs, not shell formatting."""
import copy
from pathlib import Path
import unittest
from ruamel.yaml import YAML

ROOT = Path(__file__).resolve().parents[2]


def workflow(name):
    return YAML(typ='safe').load((ROOT / '.github/workflows' / name).read_text())


def execution_steps(steps):
    for step in steps:
        yield step
        if step.get('uses', '').startswith('./'):
            action = YAML(typ='safe').load((ROOT / step['uses'] / 'action.yml').read_text())
            yield from execution_steps(action['runs'].get('steps', []))


class WorkflowTests(unittest.TestCase):
    def assert_candidate_isolated(self, document, name):
        job = document['jobs'][name]
        self.assertIn(job.get('cache-mode', document.get('cache-mode')), ('read', 'none'))
        self.assertNotIn('environment', job)
        permissions = job.get('permissions', document.get('permissions'))
        self.assertIsInstance(permissions, dict)
        self.assertTrue(all(value in ('read', 'none') for value in permissions.values()))
        for step in execution_steps(job['steps']):
            action = step.get('uses', '')
            self.assertNotIn('create-github-app-token', action)
            if action.startswith('actions/checkout@'):
                self.assertIs(step.get('with', {}).get('persist-credentials'), False)

    def test_candidate_jobs_have_restricted_cache_and_credentials(self):
        for filename, names in (
            ('preview.yml', ('quality', 'build', 'verify', 'client', 'install')),
            ('preview-client.yml', ('client',)),
        ):
            document = workflow(filename)
            for name in names:
                with self.subTest(workflow=filename, job=name):
                    self.assert_candidate_isolated(document, name)

    def test_restricted_cache_policy_accepts_inheritance_and_no_access(self):
        for mode in ('read', 'none'):
            document = workflow('preview.yml')
            document['cache-mode'] = mode
            for name in ('quality', 'build', 'verify', 'client', 'install'):
                document['jobs'][name].pop('cache-mode')
                with self.subTest(mode=mode, job=name):
                    self.assert_candidate_isolated(document, name)

    def test_candidate_policy_rejects_privilege_escalation(self):
        original = workflow('preview.yml')
        for field, value in (
            ('cache-mode', 'write'), ('cache-mode', 'write-only'),
            ('environment', 'preview-release'), ('permissions', {'contents': 'write'}),
            ('permissions', {'id-token': 'write'}),
        ):
            document = copy.deepcopy(original)
            document['jobs']['build'][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(AssertionError):
                self.assert_candidate_isolated(document, 'build')
        document = copy.deepcopy(original)
        del document['cache-mode']
        del document['jobs']['build']['cache-mode']
        with self.assertRaises(AssertionError):
            self.assert_candidate_isolated(document, 'build')
        document = copy.deepcopy(original)
        del document['permissions']
        document['jobs']['build'].pop('permissions', None)
        with self.assertRaises(AssertionError):
            self.assert_candidate_isolated(document, 'build')
        document['jobs']['build']['permissions'] = {}
        self.assert_candidate_isolated(document, 'build')

    def test_sign_test_publish_are_separate_and_publication_is_last(self):
        jobs = workflow('preview.yml')['jobs']
        self.assertTrue({'gate', 'attest', 'install'} <= set(jobs['publish']['needs']))
        self.assertTrue({'gate', 'attest'} <= set(jobs['install']['needs']))
        self.assertEqual(jobs['attest']['permissions']['id-token'], 'write')
        self.assertNotIn('id-token', jobs['publish']['permissions'])
        steps = list(execution_steps(jobs['publish']['steps']))
        evidence = next(i for i, step in enumerate(steps) if 'prepare-publication' in step.get('run', ''))
        token = next(i for i, step in enumerate(steps) if 'create-github-app-token' in step.get('uses', ''))
        self.assertLess(evidence, token)
        self.assertTrue({'client', 'attest', 'install', 'publish'} <= set(jobs['result']['needs']))


if __name__ == '__main__':
    unittest.main()
