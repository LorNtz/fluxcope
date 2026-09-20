"""Repository-independent preview protocol, shared by CI and installed clients."""
from datetime import datetime, timezone
import hashlib
import re

REPO = 'LorNtz/fluxcope'
BASE = 'master'
WORKFLOW = 'preview.yml'
SIGNER = f'{REPO}/.github/workflows/{WORKFLOW}'
RETENTION_DAYS = 30
PROFILES = dict(zip(('macos-arm64', 'macos-intel', 'linux-arm64', 'linux-x64'), (
    'aarch64-apple-darwin', 'x86_64-apple-darwin',
    'aarch64-unknown-linux-gnu', 'x86_64-unknown-linux-gnu')))
TITLE = re.compile(r'Preview #([1-9][0-9]*) ([0-9a-f]{40}) (macos-arm64|macos-intel|linux-arm64|linux-x64|all)')
MANIFEST = 'preview-manifest.json'
BUNDLE = 'preview-attestations.jsonl'
INSTALLER = 'fluxcope-preview-installer.sh'
LIMIT = 256 * 1024 * 1024


class PreviewError(RuntimeError):
    """An actionable failure; never treat it as missing remote state."""


def preview_id(value):
    if not re.fullmatch(r'[1-9][0-9]{0,19}', str(value)):
        raise PreviewError('Preview ID must be a positive workflow run ID.')
    return int(value)


def timestamp(value):
    parsed = datetime.fromisoformat(value.replace('Z', '+00:00'))
    if parsed.tzinfo is None:
        raise PreviewError('Preview timestamps must include a timezone.')
    return parsed.astimezone(timezone.utc)


def identity_from_run(current):
    match = TITLE.fullmatch(current['display_title'])
    if not match:
        raise PreviewError('Malformed preview run identity.')
    identifier = preview_id(current['id'])
    return {'schema': 1, 'channel': 'preview', 'id': identifier,
            'version': f'0.0.0-preview.{identifier}', 'tag': f'v0.0.0-preview.{identifier}',
            'repository': REPO, 'pr': int(match[1]), 'source': match[2],
            'controller': current['head_sha'], 'profile': match[3],
            'targets': list(PROFILES.values()) if match[3] == 'all' else [PROFILES[match[3]]],
            'actor': current['actor']['login'], 'created_at': current['created_at'],
            'retention_days': RETENTION_DAYS}


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def client_asset(target):
    return f'fluxcope-preview-client-{target}.tar.xz'


def platform_assets(target):
    return {f'fluxcope-{target}.tar.xz', f'fluxcope-{target}.tar.xz.sha256',
            f'{target}-build-smoke.json', f'{target}-smoke.json', f'{target}-dist-manifest.json'}


def payload_assets(identity):
    names = {'quality.json'} | set().union(*(platform_assets(t) for t in identity['targets']))
    if identity.get('format', 1) == 2:
        names |= {INSTALLER, *(client_asset(t) for t in identity['targets'])}
    return names


def public_assets(identity):
    return payload_assets(identity) | {MANIFEST, BUNDLE, 'sha256.sum'}


def identity_matches(data, identity):
    if any(data.get(key) != value for key, value in identity.items()):
        raise PreviewError('Preview evidence belongs to another source, controller, profile or run.')


def validate_manifest(manifest, identity):
    identity_matches(manifest, identity)
    if (manifest.get('format', 1) not in (1, 2) or manifest.get('result') != 'passed'
            or not re.fullmatch('[0-9a-f]{40}', manifest.get('snapshot', ''))
            or not re.fullmatch('[0-9a-f]{40}', manifest.get('tree', ''))
            or set(manifest.get('files', {})) != payload_assets(manifest)
            or any(not isinstance(value, str) or not re.fullmatch('[0-9a-f]{64}', value)
                   for value in manifest['files'].values())):
        raise PreviewError('Malformed signed preview manifest.')


def asset_limit(name):
    return LIMIT if name.endswith('.tar.xz') else 2 * 1024 * 1024
