import copy
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from dist_artifacts import digest, extract_binary
import preview_artifacts as assets
import preview_publish as publisher
from preview_identity import identity_from_run
from release_support import ReleaseError
from test_preview_identity import current_run


def fixture(directory: Path):
    identity = {**identity_from_run(current_run()), 'snapshot': 'b' * 40, 'tree': 'd' * 40, 'overlay': {}}
    target = identity['targets'][0]
    archive = f'fluxcope-{target}.tar.xz'
    (directory / archive).write_bytes(b'archive fixture')
    sha = digest(directory / archive)
    (directory / (archive + '.sha256')).write_text(sha)
    report = {'schema': 1, 'result': 'passed', 'dirty': False, 'version': identity['version'],
              'target': target, 'source': identity['snapshot'], 'archive': archive,
              'archive_sha256': sha, 'binary_sha256': 'f' * 64}
    for suffix in ('smoke', 'build-smoke'):
        (directory / f'{target}-{suffix}.json').write_text(json.dumps(report))
    dist = {'announcement_tag': identity['tag'], 'target': target, 'dist_version': assets.CONFIG['tools']['cargo-dist'],
            'artifacts': {n: {'sha256': digest(directory / n)} for n in (archive, archive + '.sha256')}}
    (directory / f'{target}-dist-manifest.json').write_text(json.dumps(dist))
    quality = {**identity, 'result': 'passed', 'package': {'version': identity['version'], 'source': identity['snapshot'], 'dirty': False}}
    (directory / 'quality.json').write_text(json.dumps(quality))
    manifest = {**identity, 'result': 'passed', 'files': {n: digest(directory / n) for n in assets.payload_assets(identity)}}
    (directory / assets.MANIFEST).write_text(json.dumps(manifest))
    (directory / assets.BUNDLE).write_text('{}')
    refresh_checksums(directory)
    return identity


def refresh_checksums(directory):
    (directory / 'sha256.sum').write_text('\n'.join(f'{digest(p)}  {p.name}' for p in sorted(directory.iterdir()) if p.name != 'sha256.sum') + '\n')


class ArtifactTests(unittest.TestCase):
    def test_exact_payload_set_and_controller_attestation(self):
        with tempfile.TemporaryDirectory() as temporary, patch.object(assets, 'provenance') as verify:
            root = Path(temporary)
            identity = fixture(root)
            sums = assets.validate_public_directory(root, identity)
            self.assertEqual(set(sums), assets.public_assets(identity))
            self.assertEqual(verify.call_count, 2)
            (root / 'unexpected.sh').write_text('no execution')
            with self.assertRaises(ReleaseError):
                assets.validate_public_directory(root, identity)
        with patch.object(assets, 'run') as command:
            assets.provenance(Path('/fixture/manifest'), identity)
            args = command.call_args.args
            self.assertEqual(args[args.index('--source-digest') + 1], identity['controller'])
            self.assertEqual(args[args.index('--source-ref') + 1], 'refs/heads/master')

    def test_resigned_checksum_cannot_hide_changed_payload_or_manifest_identity(self):
        for change in ('payload', 'identity', 'cross-target', 'binary'):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as temporary, patch.object(assets, 'provenance'):
                root = Path(temporary)
                identity = fixture(root)
                target = identity['targets'][0]
                if change == 'identity':
                    data = json.loads((root / assets.MANIFEST).read_text())
                    data['source'] = 'e' * 40
                    (root / assets.MANIFEST).write_text(json.dumps(data))
                elif change == 'payload':
                    (root / f'fluxcope-{target}.tar.xz').write_bytes(b'tampered')
                else:
                    path = root / f'{target}-smoke.json'
                    data = json.loads(path.read_text())
                    data['target' if change == 'cross-target' else 'binary_sha256'] = 'linux' if change == 'cross-target' else 'a' * 64
                    path.write_text(json.dumps(data))
                    manifest = json.loads((root / assets.MANIFEST).read_text())
                    manifest['files'][path.name] = digest(path)
                    (root / assets.MANIFEST).write_text(json.dumps(manifest))
                refresh_checksums(root)
                with self.assertRaises(ReleaseError):
                    assets.validate_public_directory(root, identity)

    def test_bounded_extraction_rejects_links_special_files_traversal_and_duplicates(self):
        for kind in ('link', 'fifo', 'path', 'duplicate', 'huge'):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                archive = root / 'test.tar'
                with tarfile.open(archive, 'w') as tar:
                    item = tarfile.TarInfo('fluxcope')
                    item.mode = 0o755
                    item.size = 2
                    tar.addfile(item, io.BytesIO(b'ok'))
                    bad = tarfile.TarInfo('../bad' if kind == 'path' else 'fluxcope' if kind == 'duplicate' else 'bad')
                    if kind == 'link':
                        bad.type = tarfile.SYMTYPE
                        bad.linkname = '/etc/passwd'
                    elif kind == 'fifo':
                        bad.type = tarfile.FIFOTYPE
                    elif kind == 'huge':
                        bad.size = 1024 * 1024 * 1024
                    if kind == 'huge':
                        # A hostile header can claim a huge body without providing it.
                        tar.fileobj.write(bad.tobuf())
                        tar.offset += tarfile.BLOCKSIZE
                    else:
                        tar.addfile(bad)
                with self.assertRaises(ReleaseError):
                    extract_binary(archive, root / 'binary')
                self.assertFalse((root / 'binary').exists())

    def test_failed_job_rerun_selects_exact_effective_attempt(self):
        current = {**current_run(), 'run_attempt': 3}
        target = 'aarch64-apple-darwin'
        job = {'conclusion': 'success', 'head_sha': current['head_sha'], 'evidence_attempt': 1}
        with tempfile.TemporaryDirectory() as temporary, patch.object(assets, 'run_jobs', return_value={'build': job}), \
                patch.object(assets, 'artifact_files', return_value={'report.json': b'{}'}) as read:
            self.assertEqual(assets.download_evidence(current, 'build', 'preview-build-' + target, {'report.json'}, Path(temporary)), 1)
            self.assertEqual(read.call_args.args[1], 'preview-build-' + target + '-1')
        with patch.object(assets, 'run_jobs', return_value={'build': {**job, 'head_sha': 'e' * 40}}):
            with self.assertRaises(ReleaseError):
                assets.require_job(current, 'build')

    def test_existing_publication_recovery_never_requires_old_build_artifacts(self):
        identity = identity_from_run(current_run())
        with tempfile.TemporaryDirectory() as temporary, \
                patch.object(publisher, 'publication_identity', return_value=(identity, current_run())), \
                patch.object(publisher, 'release_for', return_value={'draft': False}), \
                patch.object(publisher, 'download_public') as verify, \
                patch.object(publisher, 'assemble') as rebuild, patch.object(publisher, 'output') as output:
            publisher.prepare(Path(temporary))
            verify.assert_called_once()
            rebuild.assert_not_called()
            output.assert_called_once_with('existing', 'true')

    def test_existing_tag_conflict_never_pushes(self):
        identity = {**identity_from_run(current_run()), 'snapshot': 'b' * 40}
        with patch.object(publisher, 'resolve_preview_tag', return_value='e' * 40), patch.object(publisher, 'run') as push:
            with self.assertRaises(ReleaseError):
                publisher.ensure_tag(identity)
            push.assert_not_called()


if __name__ == '__main__':
    unittest.main()
