"""Boundary and lifecycle tests; native CI also exercises the actual frozen client."""
from contextlib import contextmanager
import io
import json
import os
from pathlib import Path
import subprocess
import signal
import sys
import time
import tarfile
import tempfile
import unittest
from unittest.mock import Mock, patch

from preview_client import lifecycle, model, remote, state
from preview_identity import identity_from_run
from test_preview_identity import current_run


@contextmanager
def fixture():
    with tempfile.TemporaryDirectory() as temporary:
        home = Path(temporary).resolve()
        installation = state.State(123, 'aarch64-apple-darwin', home)
        source = home / 'source'
        source.mkdir()
        identity = {**identity_from_run(current_run()), 'snapshot': 'c' * 40, 'tree': 'd' * 40, 'overlay': {}}
        target = installation.target
        binary = source / 'binary'
        binary.write_bytes(b'#!/bin/sh\nexit 0\n')
        binary.chmod(0o700)
        archive = source / f'fluxcope-{target}.tar.xz'
        with tarfile.open(archive, 'w:xz') as stream:
            stream.add(binary, arcname='fluxcope/fluxcope')
        report = {'target': target, 'source': identity['snapshot'], 'version': identity['version'],
                  'result': 'passed', 'binary_sha256': model.digest(binary)}
        (source / f'{target}-smoke.json').write_text(json.dumps(report))
        for name in model.payload_assets(identity) - {archive.name, f'{target}-smoke.json'}:
            (source / name).write_text('fixture')
        manifest = {**identity, 'result': 'passed', 'files': {name: model.digest(source / name)
                                                          for name in model.payload_assets(identity)}}
        (source / model.MANIFEST).write_text(json.dumps(manifest))
        (source / model.BUNDLE).write_text('attestation fixture')
        downloads = Mock(spec=remote.Downloads)
        def metadata(identifier, directory):
            assert identifier == identity['id']
            for name in (model.MANIFEST, model.BUNDLE):
                (directory / name).write_bytes((source / name).read_bytes())
            return manifest, {}
        downloads.metadata.side_effect = metadata
        downloads.payload.side_effect = lambda manifest, assets, name, directory: (directory / name).write_bytes((source / name).read_bytes())
        yield installation, source, manifest, downloads


class InstallationTests(unittest.TestCase):
    def test_atomic_install_offline_launch_reinstall_and_private_config(self):
        with fixture() as (installation, source, manifest, downloads), installation.lock():
            first = lifecycle.install(installation, downloads)
            self.assertFalse((installation.root / 'home').exists())
            with patch.object(lifecycle, 'run_application', return_value=0) as execute:
                lifecycle.launch(installation, downloads, port=8123)
            self.assertEqual(execute.call_args.args[2]['HOME'], str(installation.root / 'home'))
            self.assertEqual(downloads.metadata.call_count, 1)
            config = installation.root / 'home/.fluxcope/config.yml'
            original = config.read_bytes()
            self.assertEqual(config.stat().st_mode & 0o777, 0o600)
            second = lifecycle.install(installation, downloads)
            self.assertNotEqual(first, second)
            self.assertFalse(first.exists())
            self.assertEqual(config.read_bytes(), original)
            self.assertFalse((installation.home / '.fluxcope').exists())

    def test_interrupted_download_preserves_existing_install(self):
        with fixture() as (installation, source, manifest, downloads), installation.lock():
            first = lifecycle.install(installation, downloads)
            receipt = (installation.root / 'receipt.json').read_bytes()
            downloads.payload.side_effect = model.PreviewError('interrupted')
            with self.assertRaisesRegex(model.PreviewError, 'interrupted'):
                lifecycle.install(installation, downloads)
            self.assertEqual((installation.root / 'receipt.json').read_bytes(), receipt)
            self.assertEqual(lifecycle.installed(installation), first)
            self.assertEqual(list((installation.root / 'installs').iterdir()), [first])

    def test_changed_binary_or_saved_evidence_is_rejected_before_execution(self):
        for name in ('fluxcope', 'preview-manifest.json', 'aarch64-apple-darwin-smoke.json'):
            with self.subTest(name=name), fixture() as (installation, source, manifest, downloads), installation.lock():
                directory = lifecycle.install(installation, downloads)
                with (directory / name).open('ab') as stream:
                    stream.write(b' ')
                with patch.object(lifecycle, 'run_application') as execute, self.assertRaises(model.PreviewError):
                    lifecycle.launch(installation, downloads)
                execute.assert_not_called()

    def test_concurrent_install_or_launch_is_rejected(self):
        with fixture() as (installation, *_), installation.lock():
            with self.assertRaisesRegex(model.PreviewError, 'already running'), installation.lock():
                self.fail('lock admitted another process')

    def test_port_change_preserves_yaml_types_comments_and_order(self):
        with fixture() as (installation, *_), installation.lock():
            home = lifecycle.configure_port(installation, 8123)
            config = home / '.fluxcope/config.yml'
            config.write_text('# existing settings\nserver:\n  port: 8123 # port\nrecording:\n  enabled: false\nlabel: yes\nquoted: "yes"\n')
            lifecycle.configure_port(installation, 8234)
            result = config.read_text()
            self.assertIn('port: 8234 # port', result)
            self.assertIn('# existing settings', result)
            self.assertIn('label: yes', result)
            self.assertIn('quoted: "yes"', result)
            self.assertLess(result.index('recording:'), result.index('label:'))
            for port in (0, 65536, True):
                with self.assertRaises(model.PreviewError):
                    lifecycle.configure_port(installation, port)

    def test_symlinks_are_refused_during_install_config_and_removal(self):
        with fixture() as (installation, *_), installation.lock():
            outside = installation.home / 'outside'
            outside.mkdir()
            for relative in ('home', 'installs'):
                link = installation.root / relative
                link.symlink_to(outside, target_is_directory=True)
                with self.assertRaises(model.PreviewError):
                    lifecycle.remove_tree(link)
                link.unlink()
            path = installation.root / 'home/.fluxcope'
            path.mkdir(parents=True)
            config = path / 'config.yml'
            victim = outside / 'victim'
            victim.write_text('untouched')
            config.symlink_to(victim)
            with self.assertRaises(model.PreviewError):
                lifecycle.configure_port(installation, 9000)
            self.assertEqual(victim.read_text(), 'untouched')

    def test_uninstall_keeps_state_and_purge_requires_confirmation(self):
        with fixture() as (installation, source, manifest, downloads), installation.lock():
            lifecycle.install(installation, downloads)
            home = lifecycle.configure_port(installation, 8000)
            lifecycle.uninstall(installation)
            self.assertTrue((home / '.fluxcope/config.yml').exists())
            self.assertFalse((installation.root / 'installs').exists())
            with patch('builtins.open', return_value=io.StringIO('no\n')), self.assertRaises(model.PreviewError):
                lifecycle.uninstall(installation, purge=True)
            self.assertTrue(home.exists())
            terminal = Mock()
            terminal.__enter__ = Mock(return_value=terminal)
            terminal.__exit__ = Mock(return_value=False)
            terminal.readline.return_value = '123\n'
            with patch('builtins.open', return_value=terminal):
                lifecycle.uninstall(installation, purge=True)
            self.assertFalse(installation.root.exists())

    def test_bundle_extraction_rejects_links_extra_files_and_duplicates(self):
        for variant in ('link', 'extra', 'duplicate'):
            with self.subTest(variant=variant), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                archive = root / 'bundle.tar.xz'
                output = root / 'output'
                output.mkdir()
                with tarfile.open(archive, 'w:xz') as stream:
                    for name in sorted(lifecycle.CLIENT_FILES):
                        info = tarfile.TarInfo(name)
                        stream.addfile(info, io.BytesIO())
                    info = tarfile.TarInfo('gh' if variant == 'duplicate' else '../unsafe')
                    if variant == 'link':
                        info.type = tarfile.SYMTYPE
                        info.linkname = '/tmp'
                    stream.addfile(info, io.BytesIO())
                with self.assertRaises(model.PreviewError):
                    lifecycle.extract_client(archive, output)

    def test_offline_verifier_uses_bundle_custom_roots_and_no_credentials(self):
        with patch.dict(os.environ, {'GH_TOKEN': 'secret', 'GITHUB_TOKEN': 'secret', 'GIT_TOKEN': 'secret'}), \
                patch.object(remote.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)) as command:
            remote.provenance(Path('/manifest'), {'controller': 'c' * 40}, Path('/bundle'), Path('/gh'), Path('/roots'))
        self.assertIn('--custom-trusted-root', command.call_args.args[0])
        for token in ('GH_TOKEN', 'GITHUB_TOKEN', 'GIT_TOKEN'):
            self.assertNotIn(token, command.call_args.kwargs['env'])

    def test_launcher_signal_reaps_proxy_child(self):
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary).resolve()
            pidfile = home / 'child.pid'
            binary = home / 'child'
            binary.write_text(f'#!{sys.executable}\nimport os,time\nopen({str(pidfile)!r}, "w").write(str(os.getpid()))\ntime.sleep(60)\n')
            binary.chmod(0o700)
            code = ('import os,sys; from pathlib import Path; '
                    'from preview_client.lifecycle import run_application; '
                    'run_application(Path(sys.argv[1]), Path(sys.argv[2]), os.environ.copy())')
            parent = subprocess.Popen([sys.executable, '-c', code, str(binary), str(home)])
            try:
                deadline = time.monotonic() + 10
                while not pidfile.exists() and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue(pidfile.exists())
                child = int(pidfile.read_text())
                parent.send_signal(signal.SIGTERM)
                parent.wait(timeout=10)
                with self.assertRaises(ProcessLookupError):
                    os.kill(child, 0)
            finally:
                if parent.poll() is None:
                    parent.kill()
                    parent.wait(timeout=5)

    def test_manifest_accepts_legacy_and_requires_exact_new_assets(self):
        with fixture() as (_, source, manifest, downloads):
            identity = identity_from_run(current_run())
            model.validate_manifest(manifest, identity)
            manifest['format'] = 2
            with self.assertRaises(model.PreviewError):
                model.validate_manifest(manifest, identity)
            manifest['files'].update({name: 'a' * 64 for name in model.payload_assets(manifest) - manifest['files'].keys()})
            model.validate_manifest(manifest, identity)
            manifest['format'] = 3
            with self.assertRaises(model.PreviewError):
                model.validate_manifest(manifest, identity)


if __name__ == '__main__':
    unittest.main()
