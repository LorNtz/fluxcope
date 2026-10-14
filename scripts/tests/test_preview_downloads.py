import copy
import hashlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import preview_artifacts as ci
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
            with self.assertRaises(model.PreviewError):
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


class BoundedTransferTests(unittest.TestCase):
    def setUp(self):
        from test_preview_identity import current_run
        self.identity = model.identity_from_run(current_run())
        self.body = b'public bytes'

    def asset(self, name='quality.json'):
        return {'name': name, 'size': len(self.body),
                'digest': 'sha256:' + hashlib.sha256(self.body).hexdigest(),
                'browser_download_url': 'https://untrusted.invalid/payload'}

    def download(self, caller, asset, directory):
        if caller == 'ci':
            ci.download_asset(self.identity, asset, directory)
        else:
            remote.PublicTransport().download(self.identity, asset, directory)

    def opener(self, caller, body=None):
        return patch.object(ci if caller == 'ci' else remote,
                            'urlopen' if caller == 'ci' else 'public_open',
                            return_value=io.BytesIO(self.body if body is None else body))

    def test_anonymous_fixed_url_and_exact_digest(self):
        for caller in ('ci', 'client'):
            with self.subTest(caller=caller), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                with self.opener(caller) as opened:
                    self.download(caller, self.asset(), directory)
                self.assertEqual((directory / 'quality.json').read_bytes(), self.body)
                url = f"https://github.com/{model.REPO}/releases/download/{self.identity['tag']}/quality.json"
                if caller == 'ci':
                    opened.assert_called_once_with(url, timeout=60)
                else:
                    opened.assert_called_once_with(url)

    def test_unrecognized_oversized_and_missing_size_fail_before_open(self):
        variants = [
            ({'name': '../quality.json'}, model.PreviewError),
            ({'name': 'unexpected.sh'}, model.PreviewError),
            ({'size': 2 * 1024 * 1024 + 1}, model.PreviewError),
            ({'name': f"fluxcope-{self.identity['targets'][0]}.tar.xz",
              'size': model.LIMIT + 1}, model.PreviewError),
            ({'size': None}, TypeError),
        ]
        for caller in ('ci', 'client'):
            for changes, error in variants:
                with self.subTest(caller=caller, changes=changes), tempfile.TemporaryDirectory() as temporary:
                    with self.opener(caller) as opened, self.assertRaises(error):
                        self.download(caller, {**self.asset(), **changes}, Path(temporary))
                    opened.assert_not_called()
                    self.assertEqual(list(Path(temporary).iterdir()), [])
            with self.subTest(caller=caller, missing=True), tempfile.TemporaryDirectory() as temporary:
                asset = self.asset()
                del asset['size']
                with self.opener(caller) as opened, self.assertRaises(KeyError):
                    self.download(caller, asset, Path(temporary))
                opened.assert_not_called()

    def test_negative_size_preserves_caller_preflight_order(self):
        for caller in ('ci', 'client'):
            with self.subTest(caller=caller), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                with self.opener(caller) as opened, self.assertRaises(model.PreviewError):
                    self.download(caller, {**self.asset(), 'size': -1}, directory)
                self.assertEqual(opened.call_count, int(caller == 'ci'))
                self.assertEqual((directory / 'quality.json').exists(), caller == 'ci')

    def test_truncated_wrong_size_missing_or_wrong_digest_are_rejected(self):
        variants = [
            ({}, self.body[:-1]), ({'size': len(self.body) - 1}, self.body),
            ({'digest': None}, self.body), ({'digest': 'sha256:' + '0' * 64}, self.body),
        ]
        for caller in ('ci', 'client'):
            for changes, body in variants:
                with self.subTest(caller=caller, changes=changes, body=body), tempfile.TemporaryDirectory() as temporary:
                    with self.opener(caller, body), self.assertRaises(model.PreviewError):
                        self.download(caller, {**self.asset(), **changes}, Path(temporary))

    def test_running_cap_and_archive_limit_boundary(self):
        class ChunkedResponse(io.BytesIO):
            def read(self, size):
                return super().read(min(size, 4))

        name = f"fluxcope-{self.identity['targets'][0]}.tar.xz"
        for caller in ('ci', 'client'):
            for body in (self.body, self.body + b'!'):
                with self.subTest(caller=caller, length=len(body)), tempfile.TemporaryDirectory() as temporary:
                    directory = Path(temporary)
                    with patch.object(model, 'LIMIT', len(self.body)), self.opener(caller, body) as opened:
                        opened.return_value = ChunkedResponse(body)
                        if body == self.body:
                            self.download(caller, self.asset(name), directory)
                            self.assertEqual((directory / name).read_bytes(), body)
                        else:
                            with self.assertRaises(model.PreviewError):
                                self.download(caller, self.asset(name), directory)
                            self.assertEqual((directory / name).read_bytes(), self.body)

    def test_existing_and_symlink_destinations_are_never_overwritten(self):
        for caller in ('ci', 'client'):
            for kind in ('file', 'symlink', 'dangling'):
                with self.subTest(caller=caller, kind=kind), tempfile.TemporaryDirectory() as temporary:
                    directory = Path(temporary)
                    destination = directory / 'quality.json'
                    victim = directory / 'victim'
                    if kind == 'file':
                        destination.write_bytes(b'untouched')
                    else:
                        if kind == 'symlink':
                            victim.write_bytes(b'untouched')
                        destination.symlink_to(victim)
                    error = model.PreviewError if caller == 'ci' else FileExistsError
                    with self.opener(caller) as opened, self.assertRaises(error):
                        self.download(caller, self.asset(), directory)
                    self.assertEqual(opened.call_count, int(caller == 'client'))
                    if kind == 'dangling':
                        self.assertFalse(victim.exists())
                    else:
                        self.assertEqual(destination.read_bytes(), b'untouched')

    def test_ci_size_preflight_precedes_destination_guard(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / 'quality.json').write_bytes(b'untouched')
            with self.opener('ci') as opened, self.assertRaises(TypeError):
                self.download('ci', {**self.asset(), 'size': None}, directory)
            opened.assert_not_called()
            self.assertEqual((directory / 'quality.json').read_bytes(), b'untouched')


if __name__ == '__main__':
    unittest.main()
