import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from preview_client import model, remote
from test_preview_installation import fixture


class DownloadTests(unittest.TestCase):
    def test_public_identity_checks_precede_payload_execution(self):
        with fixture() as (installation, source, manifest, _):
            run = {'id': 123, 'repository': {'full_name': model.REPO}, 'event': 'workflow_dispatch',
                   'head_branch': 'master', 'workflow_id': 7, 'path': '.github/workflows/preview.yml',
                   'head_sha': manifest['controller'], 'actor': {'login': manifest['actor']},
                   'created_at': manifest['created_at'],
                   'display_title': f"Preview #{manifest['pr']} {manifest['source']} {manifest['profile']}"}
            responses = {
                'actions/runs/123': run, 'actions/workflows/preview.yml': {'id': 7},
                f"compare/{manifest['controller']}...master": {'status': 'identical'},
                f"releases/tags/{manifest['tag']}": {'draft': False, 'prerelease': True, 'immutable': True,
                    'published_at': '2099-01-01T00:00:00Z', 'assets': [
                        {'name': name, 'size': (source / name).stat().st_size if (source / name).exists() else 0,
                         'digest': 'sha256:' + manifest['files'].get(name, 'a' * 64)}
                        for name in model.public_assets(manifest)]},
                f"git/ref/tags/{manifest['tag']}": {'object': {'type': 'commit', 'sha': manifest['snapshot']}},
                f"git/commits/{manifest['snapshot']}": {'parents': [{'sha': manifest['source']}], 'tree': {'sha': manifest['tree']}},
            }
            class Transport:
                def api(self, path):
                    return responses[path]
                def download(self, identity, asset, directory):
                    (directory / asset['name']).write_bytes((source / asset['name']).read_bytes())
            downloads = remote.Downloads('/gh', transport=Transport())
            with tempfile.TemporaryDirectory() as temp, patch.object(downloads, 'verify') as verify:
                result, assets = downloads.metadata(123, Path(temp))
                self.assertEqual(result, manifest)
                self.assertEqual(set(assets), model.public_assets(manifest))
                verify.assert_called_once()
            original = copy.deepcopy(responses)
            variants = [
                ('actions/runs/123', 'head_branch', 'feature/untrusted'),
                ('actions/runs/123', 'head_sha', 'not-a-commit'),
                (f"compare/{manifest['controller']}...master", 'status', 'diverged'),
                (f"releases/tags/{manifest['tag']}", 'immutable', False),
                (f"releases/tags/{manifest['tag']}", 'published_at', '2020-01-01T00:00:00Z'),
                (f"git/ref/tags/{manifest['tag']}", 'object', {'type': 'commit', 'sha': 'f' * 40}),
                (f"git/commits/{manifest['snapshot']}", 'parents', [{'sha': 'f' * 40}]),
            ]
            for endpoint, key, value in variants:
                with self.subTest(endpoint=endpoint, key=key), tempfile.TemporaryDirectory() as temp:
                    responses = copy.deepcopy(original)
                    responses[endpoint][key] = value
                    with patch.object(downloads, 'verify'), self.assertRaises(model.PreviewError):
                        downloads.metadata(123, Path(temp))

    def test_provenance_failure_stops_metadata_processing(self):
        with patch.object(remote.subprocess, 'run') as command:
            command.return_value.returncode = 1
            command.return_value.stderr = 'wrong workflow'
            with self.assertRaisesRegex(model.PreviewError, 'provenance'):
                remote.provenance(Path('/manifest'), {'controller': 'a' * 40}, Path('/bundle'), Path('/gh'))

    def test_frozen_https_uses_portable_ca_file_and_honors_explicit_trust(self):
        import certifi
        import os
        for override, expected in ((None, certifi.where()), ('/enterprise-roots', '/enterprise-roots')):
            with self.subTest(override=override), patch.dict(os.environ, {}, clear=True), \
                    patch.object(remote.ssl, 'create_default_context') as context, patch.object(remote, 'urlopen'):
                if override:
                    os.environ['SSL_CERT_FILE'] = override
                remote.public_open('https://api.github.com/')
                context.assert_called_once_with(cafile=expected)


if __name__ == '__main__':
    unittest.main()
