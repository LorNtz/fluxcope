import json
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

    def test_verified_cached_binary_is_reused_with_isolated_home_and_port(self):
        current = current_run()
        identity = identity_from_run(current)
        target = identity['targets'][0]
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            root = home / f'.cache/fluxcope/previews/123/{target}'
            (root / 'bin').mkdir(parents=True)
            binary = root / 'bin/fluxcope'
            binary.write_bytes(b'verified executable fixture')
            report = json.dumps({'binary_sha256': digest(binary)}).encode()
            report_name = f'{target}-smoke.json'
            import hashlib
            manifest = {'files': {report_name: hashlib.sha256(report).hexdigest()}}
            def download(identity, asset, directory):
                self.assertEqual(asset['name'], report_name)
                (directory / report_name).write_bytes(report)
            with patch.object(Path, 'home', return_value=home), patch.object(preview, 'host_target', return_value=target), \
                    patch.object(preview, 'read_record', return_value={'state': 'complete'}), \
                    patch.object(preview, 'download_public', return_value=({'assets': [{'name': report_name}]}, manifest)), \
                    patch.object(preview, 'download_asset', side_effect=download), \
                    patch.object(preview, 'extract_binary') as extract, \
                    patch.object(preview.socket, 'socket') as socket, \
                    patch.object(preview.subprocess, 'run') as execute:
                socket.return_value.__enter__.return_value.getsockname.return_value = ('127.0.0.1', 45123)
                execute.return_value.returncode = 0
                preview.execute(current)
                extract.assert_not_called()
                self.assertEqual(execute.call_args.kwargs['env']['HOME'], str(root / 'home'))
                self.assertEqual(execute.call_args.kwargs['cwd'], root / 'home')
                self.assertIn('port:', (root / 'home/.fluxcope/config.yml').read_text())
                self.assertFalse((home / '.fluxcope').exists())

    def test_launcher_rejects_non_complete_or_expired_result_before_downloading(self):
        for record in (None, {'state': 'failed'}, {'state': 'expired', 'cleanup': 'complete'}):
            with self.subTest(record=record), patch.object(preview, 'read_record', return_value=record), \
                    patch.object(preview, 'download_public') as download:
                with self.assertRaises(ReleaseError):
                    preview.execute(current_run())
                download.assert_not_called()


if __name__ == '__main__':
    unittest.main()
