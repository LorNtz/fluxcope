import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from dist_artifacts import digest
import preview
from preview_identity import identity_from_run
from release_support import ReleaseError
from test_preview_identity import current_run


class ClientTests(unittest.TestCase):
    def test_cli_normalizes_all_proxy_without_overriding_explicit_choices(self):
        for environment, expected in (
            ({'ALL_PROXY': 'http://proxy:7890'}, {'http_proxy': 'http://proxy:7890', 'https_proxy': 'http://proxy:7890'}),
            ({'all_proxy': 'http://proxy:7890', 'HTTP_PROXY': 'http://other', 'https_proxy': ''}, {'https_proxy': ''}),
            ({'ALL_PROXY': 'socks5://proxy:7890'}, {}),
            ({}, {}),
        ):
            with self.subTest(environment=environment), patch.dict(os.environ, environment, clear=True), \
                    patch('sys.argv', ['preview.py', 'status', '123']), \
                    patch.object(preview, 'verify_local_repository'), \
                    patch.object(preview, 'select_run', return_value=current_run()), patch.object(preview, 'status'):
                preview.main()
                self.assertEqual({key: os.environ[key] for key in ('http_proxy', 'https_proxy') if key in os.environ}, expected)

    def test_reuses_only_active_or_unexpired_publications(self):
        active = {**current_run(), 'status': 'in_progress'}
        self.assertTrue(preview.reusable(active))
        finished = {**active, 'status': 'completed'}
        for record, release, expected in (
            ({'cleanup': 'complete'}, None, False),
            (None, None, False),
            (None, {'draft': True}, False),
            (None, {'draft': False, 'published_at': '2020-01-01T00:00:00Z'}, False),
            (None, {'draft': False, 'published_at': '2099-01-01T00:00:00Z'}, True),
        ):
            with self.subTest(record=record, release=release), patch.object(preview, 'read_record', return_value=record), \
                    patch.object(preview, 'release_for', return_value=release):
                self.assertEqual(preview.reusable(finished), expected)

    def test_run_delegates_to_installed_launcher_without_remote_metadata(self):
        from preview_client.state import State
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary).resolve()
            state = State(123, 'aarch64-apple-darwin', home)
            generation = state.root / 'installs' / ('a' * 64 + '-12345678')
            generation.mkdir(parents=True)
            (generation / 'preview-client').write_text('fixture')
            (state.root / 'receipt.json').write_text(json.dumps({'generation': generation.name}))
            state.launcher.parent.mkdir(parents=True)
            state.launcher.write_text('fixture launcher')
            with patch('preview_client.state.Path.home', return_value=home), \
                    patch('preview_client.state.host_target', return_value=state.target), \
                    patch.object(preview, 'read_record') as remote, \
                    patch.object(preview.subprocess, 'run') as execute:
                execute.return_value.returncode = 0
                preview.execute(current_run())
                remote.assert_not_called()
                self.assertEqual(execute.call_args.args[0], [str(state.launcher)])

    def test_launcher_rejects_non_complete_or_expired_result_before_downloading(self):
        for record in (None, {'state': 'failed'}, {'state': 'expired', 'cleanup': 'complete'}):
            with self.subTest(record=record), tempfile.TemporaryDirectory() as temporary, \
                    patch('preview_client.state.Path.home', return_value=Path(temporary).resolve()), \
                    patch.object(preview, 'read_record', return_value=record), \
                    patch('preview_client.lifecycle.install') as download:
                with self.assertRaises(ReleaseError):
                    preview.execute(current_run())
                download.assert_not_called()


if __name__ == '__main__':
    unittest.main()
