import os
import json
import unittest
from unittest.mock import patch

from preview_build import require_cache_mode
import preview_record
from preview_identity import identity_from_run
from release_support import ReleaseError
from test_preview_identity import current_run


class RuntimeTests(unittest.TestCase):
    def test_shell_requires_verified_probe_marker_in_actions(self):
        for mode in ('read', 'none'):
            with patch.dict(os.environ, {'GITHUB_ACTIONS': 'true', 'FLUXCOPE_PREVIEW_CACHE_MODE': mode}, clear=True):
                require_cache_mode()
        for mode in ('', 'write', 'write-only'):
            with patch.dict(os.environ, {'GITHUB_ACTIONS': 'true', 'ACTIONS_CACHE_MODE': 'read',
                                        'FLUXCOPE_PREVIEW_CACHE_MODE': mode}, clear=True):
                with self.assertRaises(ReleaseError):
                    require_cache_mode()

    def test_local_or_isolated_container_does_not_require_actions_context(self):
        with patch.dict(os.environ, {}, clear=True):
            require_cache_mode()

    def test_result_accepts_github_rewritten_display_url(self):
        identity = identity_from_run(current_run())
        record = {**identity, 'state': 'failed', 'writer_run': identity['id']}
        writer = {'id': identity['id'], 'path': '.github/workflows/preview.yml', 'event': 'workflow_dispatch'}
        check = {'details_url': 'https://github.com/LorNtz/fluxcope/runs/456',
                 'output': {'text': json.dumps(record)}}
        with patch.object(preview_record, 'owned_check', return_value=check), \
                patch.object(preview_record, 'trusted_writer', return_value=writer):
            self.assertEqual(preview_record.read_record(identity), record)
        for changed in ({**writer, 'id': 999}, {**writer, 'path': '.github/workflows/preview-cleanup.yml'}):
            with patch.object(preview_record, 'owned_check', return_value=check), \
                    patch.object(preview_record, 'trusted_writer', return_value=changed), self.assertRaises(ReleaseError):
                preview_record.read_record(identity)

    def test_cleanup_record_requires_cleanup_writer_and_consistent_state(self):
        identity = identity_from_run(current_run())
        for cleanup, state, path, valid in (
            ('pending', 'cleanup-pending', 'preview-cleanup.yml', True),
            ('complete', 'expired', 'preview-cleanup.yml', True),
            ('complete', 'complete', 'preview-cleanup.yml', False),
            ('pending', 'cleanup-pending', 'preview.yml', False),
        ):
            record = {**identity, 'state': state, 'cleanup': cleanup, 'writer_run': 999}
            with self.subTest(cleanup=cleanup, state=state, path=path), \
                    patch.object(preview_record, 'owned_check', return_value={'output': {'text': json.dumps(record)}}), \
                    patch.object(preview_record, 'trusted_writer', return_value={'path': f'.github/workflows/{path}'}):
                if valid:
                    self.assertEqual(preview_record.read_record(identity), record)
                else:
                    with self.assertRaises(ReleaseError):
                        preview_record.read_record(identity)


if __name__ == '__main__':
    unittest.main()
