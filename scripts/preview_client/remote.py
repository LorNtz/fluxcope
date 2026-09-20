"""Anonymous, bounded public transport and controller provenance verification."""
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import re
import subprocess
import ssl
import tempfile
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from .model import (BASE, BUNDLE, MANIFEST, REPO, SIGNER, TITLE, WORKFLOW,
                    PreviewError, asset_limit, digest, identity_from_run,
                    preview_id, public_assets, timestamp, validate_manifest)


def clean_environment(home):
    environment = {**os.environ, 'HOME': str(home), 'GH_CONFIG_DIR': str(home / '.gh'),
                   'GH_NO_UPDATE_NOTIFIER': '1', 'GH_PROMPT_DISABLED': '1'}
    for key in ('GH_TOKEN', 'GITHUB_TOKEN', 'GIT_TOKEN', 'GH_ENTERPRISE_TOKEN', 'GITHUB_ENTERPRISE_TOKEN'):
        environment.pop(key, None)
    return environment


def public_open(request):
    # Frozen Python cannot depend on OpenSSL paths from the packaging machine.
    # An explicit CA file supports enterprise trust and the native HTTPS replay.
    ca_file = os.environ.get('SSL_CERT_FILE')
    if not ca_file:
        try:
            import certifi
            ca_file = certifi.where()
        except ModuleNotFoundError:
            pass  # Repository-only legacy clients may use their installed Python's roots.
    return urlopen(request, timeout=60, context=ssl.create_default_context(cafile=ca_file))


class PublicTransport:
    """Only fixed public GitHub endpoints; no credentials or configurable repository."""

    def api(self, path):
        request = Request(f'https://api.github.com/repos/{REPO}/{path}', headers={
            'Accept': 'application/vnd.github+json', 'X-GitHub-Api-Version': '2022-11-28',
            'User-Agent': 'fluxcope-preview-client'})
        try:
            with public_open(request) as response:
                data = response.read(4 * 1024 * 1024 + 1)
        except HTTPError as error:
            if error.code == 403:
                raise PreviewError('GitHub public API limit reached; retry after its rate-limit reset.') from error
            if error.code == 404:
                raise PreviewError('Preview is unavailable or expired. Request a new preview ID.') from error
            raise
        if len(data) > 4 * 1024 * 1024:
            raise PreviewError('Public metadata exceeded its size limit.')
        return json.loads(data)

    def download(self, identity, asset, directory):
        name = asset['name']
        if (name not in public_assets(identity) or asset['size'] > asset_limit(name)
                or asset['size'] < 0):
            raise PreviewError('Unrecognized or oversized public preview asset.')
        path = directory / name
        url = f"https://github.com/{REPO}/releases/download/{identity['tag']}/{name}"
        size = 0
        with public_open(url) as response, path.open('xb') as stream:
            while chunk := response.read(1024 * 1024):
                size += len(chunk)
                if size > asset_limit(name):
                    raise PreviewError('Public preview download exceeded its size limit.')
                stream.write(chunk)
        if size != asset['size'] or asset.get('digest') != 'sha256:' + digest(path):
            raise PreviewError('Public preview asset differs from its GitHub digest.')


def provenance(path, identity, bundle, verifier, trusted_root=None):
    args = [str(verifier), 'attestation', 'verify', str(path), '--repo', REPO,
            '--signer-workflow', SIGNER, '--source-ref', f'refs/heads/{BASE}',
            '--source-digest', identity['controller'], '--deny-self-hosted-runners',
            '--bundle', str(bundle)]
    if trusted_root is not None:
        args += ['--custom-trusted-root', str(trusted_root)]
    with tempfile.TemporaryDirectory(prefix='fluxcope-verifier-') as temporary:
        result = subprocess.run(args, env=clean_environment(Path(temporary)),
                                text=True, capture_output=True)
    if result.returncode:
        raise PreviewError('Preview provenance verification failed: ' + result.stderr.strip())


class Downloads:
    def __init__(self, verifier, trusted_root=None, transport=None):
        self.verifier = verifier
        self.trusted_root = trusted_root
        self.transport = transport or PublicTransport()

    def verify(self, path, identity, directory):
        provenance(path, identity, directory / BUNDLE, self.verifier, self.trusted_root)

    def metadata(self, identifier, directory):
        identifier = preview_id(identifier)
        current = self.transport.api(f'actions/runs/{identifier}')
        workflow = self.transport.api(f'actions/workflows/{WORKFLOW}')
        if (current['id'] != identifier or current['repository']['full_name'] != REPO
                or current['event'] != 'workflow_dispatch' or current['head_branch'] != BASE
                or current['workflow_id'] != workflow['id']
                or current.get('path') != f'.github/workflows/{WORKFLOW}'
                or not re.fullmatch('[0-9a-f]{40}', current['head_sha'])
                or not TITLE.fullmatch(current['display_title'])):
            raise PreviewError('Run is not an authorized master preview workflow.')
        identity = identity_from_run(current)
        comparison = self.transport.api(f"compare/{identity['controller']}...{BASE}")
        if comparison['status'] not in ('ahead', 'identical'):
            raise PreviewError('Preview controller is outside protected master history.')
        release = self.transport.api(f"releases/tags/{identity['tag']}")
        if release['draft'] or not release['prerelease'] or not release.get('immutable'):
            raise PreviewError('Preview has no immutable public prerelease.')
        if datetime.now(timezone.utc) >= timestamp(release['published_at']) + timedelta(days=identity['retention_days']):
            raise PreviewError('Preview downloads expired. Request a new preview ID.')
        assets = {a['name']: a for a in release['assets']}
        if len(assets) != len(release['assets']) or not {MANIFEST, BUNDLE} <= assets.keys():
            raise PreviewError('Public preview asset set is invalid.')
        for name in (MANIFEST, BUNDLE):
            self.transport.download(identity, assets[name], directory)
        self.verify(directory / MANIFEST, identity, directory)
        manifest = json.loads((directory / MANIFEST).read_text())
        validate_manifest(manifest, identity)
        if set(assets) != public_assets(manifest):
            raise PreviewError('Public preview asset set differs from the signed format.')
        ref = self.transport.api(f"git/ref/tags/{identity['tag']}")['object']
        if ref['type'] != 'commit' or ref['sha'] != manifest['snapshot']:
            raise PreviewError('Protected preview tag differs from signed evidence.')
        commit = self.transport.api(f"git/commits/{manifest['snapshot']}")
        if ([p['sha'] for p in commit['parents']] != [identity['source']]
                or commit['tree']['sha'] != manifest['tree']):
            raise PreviewError('Preview snapshot ancestry/tree differs from signed evidence.')
        if any(assets[name].get('digest') != 'sha256:' + sha for name, sha in manifest['files'].items()):
            raise PreviewError('Public assets differ from the signed manifest.')
        return manifest, assets

    def payload(self, manifest, assets, name, directory):
        self.transport.download(manifest, assets[name], directory)
        if digest(directory / name) != manifest['files'][name]:
            raise PreviewError('Downloaded payload differs from the signed manifest.')
