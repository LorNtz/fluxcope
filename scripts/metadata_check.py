#!/usr/bin/env python3
"""Fail on drift between reviewed release metadata, tool pins and time-limited exceptions."""
from datetime import date
import json
from pathlib import Path
import tomllib
from release_gate import package_identity
from release_support import CONFIG, ROOT, validate_intent


def main():
    package_identity((ROOT / 'Cargo.toml').read_bytes(), (ROOT / 'Cargo.lock').read_bytes())
    manifest = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']
    assert manifest['repository'] == f"https://github.com/{CONFIG['repository']}"
    assert manifest['license'] == 'MIT OR Apache-2.0'
    assert tomllib.loads((ROOT / 'rust-toolchain.toml').read_text())['toolchain']['channel'] == CONFIG['tools']['rust']
    dist = tomllib.loads((ROOT / 'dist-workspace.toml').read_text())['dist']
    assert dist['ci'] == []
    assert set(dist['targets']) == {p['target'] for p in CONFIG['platforms']}
    assert dist['cargo-dist-version'] == CONFIG['tools']['cargo-dist']
    for name, pin in json.loads((ROOT / '.github/tool-pins.json').read_text()).items():
        assert pin['version'] == CONFIG['tools'][name]
    for target in CONFIG['platforms']:
        if 'container' in target:
            assert '@sha256:' in target['container']
        else:
            assert target['runner'] in ('macos-15', 'macos-15-intel') and target['minimum_macos'] == 15
    exceptions = json.loads((ROOT / '.github/advisory-exceptions.json').read_text())
    deny = tomllib.loads((ROOT / 'deny.toml').read_text())
    assert {item['id'] for item in deny['advisories']['ignore']} == set(exceptions)
    for advisory, exception in exceptions.items():
        assert date.today() <= date.fromisoformat(exception['expires']), f'Expired exception: {advisory}'
        assert exception['reason'] and exception['version']
    intent = ROOT / '.github/release-intent.json'
    if intent.exists():
        validate_intent(json.loads(intent.read_text()), manifest['version'])
    print('Release metadata, platform matrix, tool pins and exception expiry agree.')


if __name__ == '__main__':
    main()
