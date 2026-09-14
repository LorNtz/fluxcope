"""Security boundaries use synthetic GitHub/registry facts; no network or side effects."""
from copy import deepcopy
import unittest
from unittest.mock import patch
import release_gate as gate
import release_support as support


class ReleaseIdentityTests(unittest.TestCase):
    def setUp(self):
        self.intent = {'schema': 1, 'channel': 'stable', 'mode': 'explicit', 'version': '0.1.0', 'previous_stable': None}
        self.pr = {'base': {'ref': support.BASE, 'repo': {'full_name': support.REPO}},
                   'head': {'ref': 'release-plz-2026', 'sha': 'a'*40, 'repo': {'full_name': support.REPO}},
                   'user': {'login': support.CONFIG['release_authors'][0]}, 'labels': [{'name': 'release'}]}

    def test_version_inputs_cannot_escape_tag_or_command_boundaries(self):
        for value in ('v0.1.0', '0.1.0\n', '0.1.0;echo hi', '0.1.0-rc.0', '01.1.0', '--help', '0.1.0+metadata'):
            with self.subTest(value=value), self.assertRaises(support.ReleaseError):
                support.version(value)
        self.assertEqual(support.version('1.2.3-rc.4'), '1.2.3-rc.4')

    def test_fork_or_wrong_actor_cannot_become_a_release_pr(self):
        self.assertTrue(support.eligible_pr(self.pr))
        for mutation in (
            lambda p: p['head']['repo'].update(full_name='attacker/fork'),
            lambda p: p['base'].update(ref='feature'),
            lambda p: p['user'].update(login='untrusted-bot'),
            lambda p: p.update(labels=[]),
            lambda p: p['head'].update(ref='feature/release'),
        ):
            candidate = deepcopy(self.pr)
            mutation(candidate)
            self.assertFalse(support.eligible_pr(candidate))

    def test_rc_intent_cannot_enable_stable_channel(self):
        self.intent['version'] = '0.2.0-rc.1'
        with self.assertRaises(support.ReleaseError):
            support.validate_intent(self.intent)

    def test_lockfile_disagreement_and_registry_sourced_root_are_rejected(self):
        manifest = b'[package]\nname="fluxcope"\nversion="0.1.0"\n'
        for lock in (b'[[package]]\nname="fluxcope"\nversion="0.2.0"\n',
                     b'[[package]]\nname="fluxcope"\nversion="0.1.0"\nsource="registry+evil"\n'):
            with self.assertRaises(support.ReleaseError):
                gate.package_identity(manifest, lock)

    def test_api_failure_is_not_treated_as_an_absent_tag(self):
        for failure in ('network failure', 'rate limit (HTTP 403)', 'unauthorized (HTTP 401)'):
            with patch.object(gate, 'api', side_effect=support.ReleaseError(failure)):
                with self.assertRaises(support.ReleaseError):
                    gate.resolve_tag('0.1.0')
        with patch.object(gate, 'api', side_effect=support.ReleaseError('Not Found (HTTP 404)')):
            self.assertIsNone(gate.resolve_tag('0.1.0'))

    def test_terminal_text_does_not_emit_escape_sequences(self):
        self.assertNotIn('\x1b', support.safe_text('\x1b[31mhello'))
        self.assertNotIn('\x9b', support.safe_text('\x9b31mhello'))


if __name__ == '__main__':
    unittest.main()
