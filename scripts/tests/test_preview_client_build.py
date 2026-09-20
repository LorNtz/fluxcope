from pathlib import Path
import unittest
from unittest.mock import patch

import preview_client_build as builder
from preview_client.model import PreviewError


class ClientBuildTests(unittest.TestCase):
    def test_macos_legacy_and_modern_minimums(self):
        for commands, valid in (
            ('cmd LC_VERSION_MIN_MACOSX\n cmdsize 16\n version 10.13\n sdk 26.2', True),
            ('cmd LC_BUILD_VERSION\n minos 15.0\n sdk 26.2', True),
            ('cmd LC_BUILD_VERSION\n minos 15.1\n sdk 26.2', False),
            ('cmd LC_VERSION_MIN_MACOSX\n cmdsize 16\n version 16.0\n sdk 26.2', False),
            ('cmd LC_SOURCE_VERSION\n version 10.13', False),
        ):
            with self.subTest(commands=commands), patch.object(builder, 'run', return_value=commands):
                if valid:
                    builder.client_contract(Path('/client'), 'x86_64-apple-darwin')
                else:
                    with self.assertRaises(PreviewError):
                        builder.client_contract(Path('/client'), 'x86_64-apple-darwin')

    def test_static_verifier_does_not_require_glibc_symbols(self):
        for headers, dynamic in (('LOAD', False), ('INTERP\nLOAD', True)):
            with patch.object(builder, 'run', return_value=headers), patch.object(builder, 'binary_contract') as contract:
                builder.client_contract(Path('/gh'), 'x86_64-unknown-linux-gnu')
                self.assertEqual(contract.called, dynamic)
