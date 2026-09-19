"""Keep trusted smoke verification compatible with stable and preview log layouts."""
from pathlib import Path
import tempfile
import unittest

from smoke import smoke_log


class SmokeLogTests(unittest.TestCase):
    def test_selects_the_single_log_from_either_layout(self):
        for relative in ('fluxcope.log', 'logs/endpoint.log'):
            with self.subTest(layout=relative), tempfile.TemporaryDirectory() as directory:
                state = Path(directory)
                expected = state / relative
                expected.parent.mkdir(exist_ok=True)
                expected.write_text('CA certificate download URL: fixture\n')
                self.assertEqual(smoke_log(state), expected)

    def test_rejects_missing_or_ambiguous_logs(self):
        for files in ((), ('logs/first.log', 'logs/second.log'),
                      ('fluxcope.log', 'logs/endpoint.log')):
            with self.subTest(files=files), tempfile.TemporaryDirectory() as directory:
                state = Path(directory)
                for relative in files:
                    path = state / relative
                    path.parent.mkdir(exist_ok=True)
                    path.touch()
                with self.assertRaisesRegex(AssertionError, 'expected exactly one app log'):
                    smoke_log(state)


if __name__ == '__main__':
    unittest.main()
