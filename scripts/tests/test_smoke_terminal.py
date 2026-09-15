"""Exercise independent keystrokes through a real PTY with immediate redraws."""
from pathlib import Path
import sys
import tempfile
import unittest

from smoke import Terminal


class TerminalInputTests(unittest.TestCase):
    def test_redraw_does_not_join_escape_with_the_next_key(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            child = home / 'terminal-fixture'
            child.write_text(f'#!{sys.executable}\n' + '''import os, select, tty
tty.setraw(0)
os.write(1, b'ready')
first = os.read(0, 1)
# The redraw deliberately wakes the parent's output drain immediately.
os.write(1, b'redraw')
if select.select([0], [], [], 0.06)[0]:
    first += os.read(0, 1)
os.write(1, b'first=' + first.hex().encode() + b'\\n')
second = os.read(0, 1)
os.write(1, b'second=' + second.hex().encode() + b'\\n')
''')
            child.chmod(0o700)
            with Terminal(child, home) as terminal:
                terminal.wait(lambda: b'ready' in terminal.output, 'fixture startup')
                terminal.key(b'\x1b')
                terminal.key(b'q')
                terminal.wait(lambda: b'second=' in terminal.output, 'independent keys')
                self.assertIn(b'first=1b\n', terminal.output)
                self.assertIn(b'second=71\n', terminal.output)


if __name__ == '__main__':
    unittest.main()
