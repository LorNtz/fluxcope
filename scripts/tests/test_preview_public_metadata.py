"""CI/client protocol parity through real metadata and download entry points."""
from contextlib import contextmanager, ExitStack
from datetime import datetime, timedelta, timezone
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import preview_artifacts as ci
from preview_client import model, remote
from test_preview_artifacts import fixture, refresh_checksums
from test_preview_identity import current_run


class PublicFixture:
    def __init__(self, root, format_version=None):
        self.source = root / 'source'
        self.source.mkdir()
        fixture(self.source)
        self.identity = model.identity_from_run(current_run())
        self.manifest = json.loads((self.source / model.MANIFEST).read_text())
        if format_version is not None:
            self.manifest['format'] = format_version
        if format_version == 2:
            for name in (model.INSTALLER, model.client_asset(self.identity['targets'][0])):
                (self.source / name).write_bytes(b'controller-owned payload')
            self.manifest['files'] = {name: model.digest(self.source / name)
                                      for name in model.payload_assets(self.manifest)}
        self.publish_manifest()
        self.now = datetime(2026, 10, 14, tzinfo=timezone.utc)
        self.release = {'draft': False, 'prerelease': True, 'immutable': True,
                        'published_at': self.now.isoformat(), 'assets': []}
        self.refresh_assets()
        current = {**current_run(), 'repository': {'full_name': model.REPO},
                   'event': 'workflow_dispatch', 'head_branch': model.BASE,
                   'workflow_id': 7, 'path': '.github/workflows/preview.yml'}
        self.ref = {'object': {'type': 'commit', 'sha': self.manifest['snapshot']}}
        self.commit = {'parents': [{'sha': self.identity['source']}],
                       'tree': {'sha': self.manifest['tree']}}
        self.responses = {
            'actions/runs/123': current, 'actions/workflows/preview.yml': {'id': 7},
            f"compare/{self.identity['controller']}...master": {'status': 'identical'},
            f"releases/tags/{self.identity['tag']}": self.release,
            f"git/ref/tags/{self.identity['tag']}": self.ref,
            f"git/commits/{self.manifest['snapshot']}": self.commit,
        }
        self.events = []
        self.provenance_error = None

    def publish_manifest(self):
        (self.source / model.MANIFEST).write_text(json.dumps(self.manifest))
        refresh_checksums(self.source)

    def refresh_assets(self):
        self.release['assets'] = [
            {'name': path.name, 'size': path.stat().st_size,
             'digest': 'sha256:' + model.digest(path)}
            for path in sorted(self.source.iterdir())]

    def api(self, endpoint):
        endpoint = endpoint.removeprefix(f'repos/{model.REPO}/')
        self.events.append(('api', endpoint))
        return self.responses[endpoint]

    def open(self, request, **kwargs):
        if not isinstance(request, str):
            return io.BytesIO(json.dumps(self.api(request.full_url.removeprefix(
                f'https://api.github.com/repos/{model.REPO}/'))).encode())
        prefix = f"https://github.com/{model.REPO}/releases/download/{self.identity['tag']}/"
        if not request.startswith(prefix):
            raise AssertionError(f'Unexpected download URL: {request}')
        name = request.removeprefix(prefix)
        self.events.append(('download', name))
        return io.BytesIO((self.source / name).read_bytes())

    def provenance(self, path, *args, **kwargs):
        self.events.append(('provenance', path.name))
        if self.provenance_error:
            raise model.PreviewError(self.provenance_error)

    @contextmanager
    def connected(self):
        with ExitStack() as stack:
            stack.enter_context(patch.object(ci, 'optional_api', side_effect=self.api))
            stack.enter_context(patch.object(ci, 'api', side_effect=self.api))
            stack.enter_context(patch.object(ci, 'urlopen', side_effect=self.open))
            stack.enter_context(patch.object(remote, 'public_open', side_effect=self.open))
            for module in (ci, remote):
                stack.enter_context(patch.object(module, 'provenance', side_effect=self.provenance))
                clock = stack.enter_context(patch.object(module, 'datetime'))
                clock.now.return_value = self.now
            yield

    def metadata(self, caller, directory, **kwargs):
        if caller == 'ci':
            return ci.download_public(self.identity, directory, metadata_only=True, **kwargs)[1]
        directory.mkdir()
        return remote.Downloads('/bundled/gh').metadata(123, directory)[0]


class PublicMetadataTests(unittest.TestCase):
    def test_formats_one_two_and_legacy_omission_are_accepted(self):
        for caller in ('ci', 'client'):
            for version in (None, 1, 2):
                with self.subTest(caller=caller, version=version), tempfile.TemporaryDirectory() as temporary:
                    root = Path(temporary)
                    public = PublicFixture(root, version)
                    with public.connected():
                        self.assertEqual(public.metadata(caller, root / 'download'), public.manifest)
                    self.assertEqual([event for event in public.events if event[0] == 'download'],
                                     [('download', model.MANIFEST), ('download', model.BUNDLE)])
                    self.assertLess(public.events.index(('provenance', model.MANIFEST)),
                                    public.events.index(('api', f"git/ref/tags/{public.identity['tag']}")))

    def test_immutable_prerelease_and_expiry_boundary_precede_downloads(self):
        variants = [('draft', True), ('prerelease', False), ('immutable', False),
                    ('immutable', None), ('expired', 0), ('expired', -1)]
        for caller in ('ci', 'client'):
            for key, value in variants:
                with self.subTest(caller=caller, key=key, value=value), tempfile.TemporaryDirectory() as temporary:
                    root = Path(temporary)
                    public = PublicFixture(root)
                    if key == 'expired':
                        public.release['published_at'] = (public.now - timedelta(days=30)
                                                          + timedelta(microseconds=value)).isoformat()
                    else:
                        public.release[key] = value
                    with public.connected(), self.assertRaises(model.PreviewError):
                        public.metadata(caller, root / 'download')
                    self.assertFalse(any(event[0] in ('download', 'provenance') for event in public.events))
        for caller in ('ci', 'client'):
            with self.subTest(caller=caller, before_boundary=True), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                public = PublicFixture(root)
                public.release['published_at'] = (public.now - timedelta(days=30)
                                                  + timedelta(microseconds=1)).isoformat()
                with public.connected():
                    self.assertEqual(public.metadata(caller, root / 'download'), public.manifest)

    def test_duplicate_missing_and_malformed_assets_fail_before_downloads(self):
        for caller in ('ci', 'client'):
            for variant in ('duplicate', 'manifest', 'bundle', 'malformed'):
                with self.subTest(caller=caller, variant=variant), tempfile.TemporaryDirectory() as temporary:
                    root = Path(temporary)
                    public = PublicFixture(root)
                    if variant == 'duplicate':
                        public.release['assets'].append(public.release['assets'][0].copy())
                    elif variant == 'malformed':
                        public.release['assets'].append({'size': 1})
                    else:
                        missing = model.MANIFEST if variant == 'manifest' else model.BUNDLE
                        public.release['assets'] = [a for a in public.release['assets'] if a['name'] != missing]
                    error = KeyError if variant == 'malformed' else model.PreviewError
                    with public.connected(), self.assertRaises(error):
                        public.metadata(caller, root / 'download')
                    self.assertFalse(any(event[0] == 'download' for event in public.events))

    def test_signed_manifest_identity_format_shape_and_asset_allowlist(self):
        variants = [('id', 999), ('controller', 'f' * 40), ('format', 3),
                    ('result', 'failed'), ('snapshot', 'invalid'), ('tree', 'invalid'),
                    ('files', {}), ('digest', 'invalid'), ('extra', None), ('missing', None)]
        for caller in ('ci', 'client'):
            for key, value in variants:
                with self.subTest(caller=caller, key=key), tempfile.TemporaryDirectory() as temporary:
                    root = Path(temporary)
                    public = PublicFixture(root)
                    if key == 'digest':
                        public.manifest['files']['quality.json'] = value
                    elif key not in ('extra', 'missing'):
                        public.manifest[key] = value
                    public.publish_manifest()
                    public.refresh_assets()
                    if key == 'extra':
                        public.release['assets'].append({'name': 'unexpected.sh', 'size': 0})
                    elif key == 'missing':
                        public.release['assets'] = [a for a in public.release['assets'] if a['name'] != 'quality.json']
                    with public.connected(), self.assertRaises(model.PreviewError):
                        public.metadata(caller, root / 'download')
                    self.assertIn(('provenance', model.MANIFEST), public.events)
                    self.assertFalse(any(event[0] == 'api' and event[1].startswith('git/')
                                         for event in public.events))

    def test_provenance_rejects_before_parsing_untrusted_manifest(self):
        for caller in ('ci', 'client'):
            with self.subTest(caller=caller), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                public = PublicFixture(root)
                (public.source / model.MANIFEST).write_bytes(b'not JSON')
                public.refresh_assets()
                public.provenance_error = 'untrusted controller provenance'
                with public.connected(), self.assertRaises(model.PreviewError):
                    public.metadata(caller, root / 'download')
                self.assertEqual(public.events[-1], ('provenance', model.MANIFEST))

    def test_tag_snapshot_tree_ancestry_and_signed_payload_digests(self):
        for caller in ('ci', 'client'):
            for variant in ('annotated', 'tag', 'parent', 'merge', 'root', 'tree', 'digest', 'missing-digest'):
                with self.subTest(caller=caller, variant=variant), tempfile.TemporaryDirectory() as temporary:
                    root = Path(temporary)
                    public = PublicFixture(root)
                    if variant == 'annotated':
                        public.ref['object']['type'] = 'tag'
                    elif variant == 'tag':
                        public.ref['object']['sha'] = 'f' * 40
                    elif variant == 'parent':
                        public.commit['parents'] = [{'sha': 'f' * 40}]
                    elif variant == 'merge':
                        public.commit['parents'].append({'sha': 'f' * 40})
                    elif variant == 'root':
                        public.commit['parents'] = []
                    elif variant == 'tree':
                        public.commit['tree']['sha'] = 'f' * 40
                    else:
                        asset = next(a for a in public.release['assets'] if a['name'] == 'quality.json')
                        if variant == 'digest':
                            asset['digest'] = 'sha256:' + 'f' * 64
                        else:
                            del asset['digest']
                    with public.connected(), self.assertRaises(model.PreviewError):
                        public.metadata(caller, root / 'download')
                    self.assertIn(('provenance', model.MANIFEST), public.events)
                    self.assertEqual([event for event in public.events if event[0] == 'download'],
                                     [('download', model.MANIFEST), ('download', model.BUNDLE)])

    def test_ci_cleanup_can_read_expired_metadata_without_payloads(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            public = PublicFixture(root)
            public.release['published_at'] = '2020-01-01T00:00:00Z'
            with public.connected():
                self.assertEqual(public.metadata('ci', root / 'download', allow_expired=True), public.manifest)
            self.assertEqual({p.name for p in (root / 'download').iterdir()}, {model.MANIFEST, model.BUNDLE})

    def test_ci_full_download_keeps_payload_report_verification(self):
        for corrupt in (False, True):
            with self.subTest(corrupt=corrupt), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                public = PublicFixture(root, 2)
                if corrupt:
                    name = f"{public.identity['targets'][0]}-smoke.json"
                    report = json.loads((public.source / name).read_text())
                    report['result'] = 'failed'
                    (public.source / name).write_text(json.dumps(report))
                    public.manifest['files'][name] = model.digest(public.source / name)
                    public.publish_manifest()
                    public.refresh_assets()
                with public.connected():
                    if corrupt:
                        with self.assertRaises(model.PreviewError):
                            ci.download_public(public.identity, root / 'download')
                    else:
                        release, manifest = ci.download_public(public.identity, root / 'download')
                        self.assertEqual(release, public.release)
                        self.assertEqual(manifest, public.manifest)
                        self.assertEqual({p.name for p in (root / 'download').iterdir()}, model.public_assets(manifest))
                        self.assertIn(('provenance', f"fluxcope-{public.identity['targets'][0]}.tar.xz"), public.events)


if __name__ == '__main__':
    unittest.main()
