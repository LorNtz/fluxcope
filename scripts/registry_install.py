#!/usr/bin/env python3
"""Install the exact published registry version without publication credentials."""
import argparse
import json
from pathlib import Path
import tempfile

from release_evidence import source_package
from release_gate import dispatch_gate
from release_source import check_registry
from release_support import CONFIG, ROOT, ReleaseError, run


def install(identity: dict):
    if identity['channel'] != 'stable':
        raise ReleaseError('Registry installation is not applicable to RC releases.')
    package, _ = source_package(identity)
    check_registry(identity, package['sha256'])
    with tempfile.TemporaryDirectory(prefix='fluxcope-registry-install-') as directory:
        installed = Path(directory)
        run('cargo', 'install', CONFIG['crate'], '--version', '=' + identity['version'], '--locked',
            '--target-dir', str(ROOT / 'target/registry-install'), '--root', str(installed), capture=False)
        report = ROOT / 'target/registry-install.json'
        run('python3', 'scripts/smoke.py', '--binary', str(installed / 'bin/fluxcope'),
            '--version', identity['version'], '--target', 'x86_64-unknown-linux-gnu', '--report', str(report), capture=False)
        result = json.loads(report.read_text())
        result.update(source=identity['source'], crate_sha256=package['sha256'])
        report.write_text(json.dumps(result, indent=2) + '\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tag', required=True)
    install(dispatch_gate(parser.parse_args().tag))
