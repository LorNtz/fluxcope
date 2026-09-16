from datetime import datetime, timedelta, timezone
import unittest
from unittest.mock import patch

import preview_cleanup as cleanup
from preview_identity import identity_from_run
from release_support import ReleaseError
from test_preview_identity import current_run


class CleanupTests(unittest.TestCase):
    def test_retention_is_measured_from_publication_or_draft_creation(self):
        now = datetime(2026, 10, 16, tzinfo=timezone.utc)
        release = {'draft': False, 'created_at': '2026-09-01T00:00:00Z', 'published_at': '2026-09-17T00:00:00Z'}
        self.assertFalse(cleanup.expired(release, 30, now))
        self.assertTrue(cleanup.expired(release, 30, now + timedelta(days=1)))
        self.assertTrue(cleanup.expired({**release, 'draft': True}, 30, now))

    def test_missing_release_is_not_assumed_to_have_expired(self):
        with patch.object(cleanup, 'require_cleanup_context'), \
                patch.object(cleanup, 'trusted_run', return_value={**current_run(), 'status': 'completed'}), \
                patch.object(cleanup, 'read_record', return_value=None), patch.object(cleanup, 'release_for', return_value=None), \
                patch.object(cleanup, 'write_record') as write:
            with self.assertRaises(ReleaseError):
                cleanup.cleanup(123)
            write.assert_not_called()

    def test_lost_delete_response_reconciles_pending_record(self):
        record = {'cleanup': 'pending', 'snapshot': 'b' * 40, 'former_url': 'https://example.invalid/preview'}
        with patch.object(cleanup, 'require_cleanup_context'), \
                patch.object(cleanup, 'trusted_run', return_value={**current_run(), 'status': 'completed'}), \
                patch.object(cleanup, 'read_record', return_value=record), patch.object(cleanup, 'release_for', return_value=None), \
                patch.object(cleanup, 'resolve_preview_tag', return_value=record['snapshot']), \
                patch.object(cleanup, 'write_record') as write, patch.object(cleanup, 'api') as mutate:
            cleanup.cleanup(123)
            self.assertEqual(write.call_args.args[1], 'expired')
            self.assertEqual(write.call_args.kwargs['cleanup'], 'complete')
            mutate.assert_not_called()

    def test_discovery_reconciles_second_page_and_ignores_rejected_manual_input(self):
        newer = {**current_run(), 'created_at': '2099-01-01T00:00:00Z'}
        older = {**current_run(), 'id': 99, 'created_at': '2020-01-01T00:00:00Z'}
        responses = [{'workflow_runs': [{'display_title': 'Preview #oops bad-sha all'}, *[newer] * 99]},
                     {'workflow_runs': [older]}]
        with patch.object(cleanup, 'require_cleanup_context'), patch.object(cleanup, 'pages', return_value=[]), \
                patch.object(cleanup, 'api', side_effect=responses) as api, \
                patch.object(cleanup, 'read_record', return_value={'cleanup': 'pending'}) as read, \
                patch.object(cleanup, 'output') as output:
            cleanup.discover()
            self.assertIn('page=2', api.call_args.args[0])
            read.assert_called_once()
            output.assert_any_call('ids', [99])

    def test_cleanup_records_intent_before_deleting_and_never_deletes_tags(self):
        identity = identity_from_run(current_run())
        release = {'id': 55, 'draft': False, 'tag_name': identity['tag'], 'published_at': '2020-01-01T00:00:00Z',
                   'created_at': '2020-01-01T00:00:00Z', 'html_url': 'https://example.invalid/preview'}
        operations = []
        def api(endpoint, **kwargs):
            operations.append((endpoint, kwargs.get('method', 'GET')))
            return release
        def write(*args, **kwargs):
            operations.append(('record', kwargs['cleanup']))
        with patch.object(cleanup, 'require_cleanup_context'), \
                patch.object(cleanup, 'trusted_run', return_value={**current_run(), 'status': 'completed'}), \
                patch.object(cleanup, 'read_record', return_value=None), patch.object(cleanup, 'release_for', side_effect=[release, None]), \
                patch.object(cleanup, 'download_public', return_value=(release, {'snapshot': 'b' * 40})), \
                patch.object(cleanup, 'resolve_preview_tag', return_value='b' * 40), \
                patch.object(cleanup, 'write_record', side_effect=write), patch.object(cleanup, 'api', side_effect=api):
            cleanup.cleanup(123)
        self.assertLess(operations.index(('record', 'pending')), operations.index(('repos/LorNtz/fluxcope/releases/55', 'DELETE')))
        self.assertEqual(operations[-1], ('record', 'complete'))
        self.assertFalse(any('git/ref' in endpoint for endpoint, _ in operations))


if __name__ == '__main__':
    unittest.main()
