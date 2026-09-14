"""The irreversible bootstrap upload must not request a token before source checks."""
from contextlib import ExitStack
import json
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import bootstrap_publish as bootstrap
import release_prepare as prepare
from release_support import ReleaseError


class BootstrapTests(unittest.TestCase):
    def setUp(self):
        self.identity = {'source': 'a' * 40, 'version': '0.1.0', 'channel': 'stable', 'pr': 7, 'url': 'pr'}
        self.package = {'source': self.identity['source'], 'version': '0.1.0', 'dirty': False, 'ci_run': 1, 'sha256': 'b' * 64}
        self.actual = dict(self.package)
        self.uploaded = []
        self.result = 0

    def run_local(self, *args, **kwargs):
        if args[0] == 'python3':
            report = kwargs['cwd'] / 'target/package-report.json'
            report.parent.mkdir(parents=True)
            report.write_text(json.dumps(self.actual))
        return ''

    def upload(self, args, **kwargs):
        self.uploaded.append((args, dict(kwargs['env'])))
        return SimpleNamespace(returncode=self.result)

    def environment(self):
        stack = ExitStack()
        for target, attribute, options in [
            (bootstrap.sys.stdin, 'isatty', {'return_value': True}), (bootstrap.sys.stdout, 'isatty', {'return_value': True}),
            (bootstrap, 'authorized_version', {'return_value': self.identity}),
            (bootstrap, 'api', {'return_value': {'value': 'false'}}),
            (bootstrap, 'source_package', {'return_value': (self.package, b'crate')}),
            (bootstrap, 'registry_version', {'return_value': None}), (bootstrap, 'resolve_tag', {'return_value': None}),
            (prepare, 'previous_stable', {'return_value': None}), (bootstrap, 'run', {'side_effect': self.run_local}),
            (bootstrap.subprocess, 'run', {'side_effect': self.upload}),
        ]:
            stack.enter_context(patch.object(target, attribute, **options))
        return stack

    def test_package_mismatch_does_not_prompt_for_token_or_upload(self):
        self.actual['sha256'] = 'c' * 64
        with self.environment(), patch.object(bootstrap.getpass, 'getpass') as prompt, patch('builtins.print'), self.assertRaises(ReleaseError):
            bootstrap.publish('0.1.0', 7)
        prompt.assert_not_called()
        self.assertFalse(self.uploaded)

    def test_retry_of_confirmed_upload_verifies_registry_without_token(self):
        with self.environment(), patch.object(bootstrap, 'registry_version', return_value={'checksum': self.package['sha256']}), \
                patch.object(bootstrap, 'check_registry') as check, patch.object(bootstrap.getpass, 'getpass') as prompt, patch('builtins.print'):
            bootstrap.publish('0.1.0', 7)
        check.assert_called_once()
        prompt.assert_not_called()
        self.assertFalse(self.uploaded)

    def test_success_failure_and_interrupt_remind_revocation_and_keep_token_out_of_arguments(self):
        for result in ('success', 'upload-failed', 'verification-failed', 'interrupted'):
            self.uploaded.clear()
            self.result = 1 if result == 'upload-failed' else 0
            error = KeyboardInterrupt() if result == 'interrupted' else (ReleaseError('digest conflict') if result == 'verification-failed' else None)
            with self.subTest(result=result), self.environment(), patch('builtins.input', return_value='0.1.0'), \
                    patch.object(bootstrap.getpass, 'getpass', return_value='test-not-a-credential'), \
                    patch.object(bootstrap, 'check_registry', side_effect=error), patch('builtins.print') as printed:
                if result == 'success':
                    bootstrap.publish('0.1.0', 7)
                else:
                    with self.assertRaises((ReleaseError, KeyboardInterrupt)):
                        bootstrap.publish('0.1.0', 7)
            self.assertTrue(any('Revoke the one-time token' in str(call) for call in printed.call_args_list))
            args, env = self.uploaded[0]
            self.assertNotIn('test-not-a-credential', args)
            self.assertEqual(env['CARGO_REGISTRY_TOKEN'], 'test-not-a-credential')


if __name__ == '__main__':
    unittest.main()
