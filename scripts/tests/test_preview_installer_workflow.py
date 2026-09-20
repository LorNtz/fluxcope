"""Check the installer trust boundary across CI jobs, not shell formatting."""
from pathlib import Path
import unittest
from ruamel.yaml import YAML

ROOT = Path(__file__).resolve().parents[2]


class WorkflowTests(unittest.TestCase):
    def test_sign_test_publish_are_separate_and_publication_is_last(self):
        jobs = YAML(typ='safe').load((ROOT / '.github/workflows/preview.yml').read_text())['jobs']
        self.assertEqual(set(jobs['publish']['needs']), {'gate', 'attest', 'install'})
        self.assertEqual(set(jobs['install']['needs']), {'gate', 'attest'})
        self.assertEqual(jobs['attest']['permissions']['id-token'], 'write')
        self.assertNotIn('id-token', jobs['publish']['permissions'])
        for name in ('client', 'install'):
            job = jobs[name]
            self.assertNotIn('environment', job)
            self.assertEqual(job['cache-mode'], 'read')
            self.assertFalse(any('create-github-app-token' in step.get('uses', '') for step in job['steps']))
        steps = jobs['publish']['steps']
        evidence = next(i for i, step in enumerate(steps) if 'prepare-publication' in step.get('run', ''))
        token = next(i for i, step in enumerate(steps) if 'create-github-app-token' in step.get('uses', ''))
        self.assertLess(evidence, token)
        self.assertTrue({'client', 'attest', 'install', 'publish'} <= set(jobs['result']['needs']))


if __name__ == '__main__':
    unittest.main()
