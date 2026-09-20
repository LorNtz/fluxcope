"""Preview payload validation and signed public downloads; never execute artifacts."""
from __future__ import annotations

from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
import re
from urllib.request import urlopen

from dist_artifacts import digest
from preview_identity import BASE, CONFIG, REPO, SIGNER, timestamp
from release_evidence import artifact_files
from release_gate import optional_api
from release_publish import checksums
from release_support import ReleaseError, api, pages, repository_path, run, run_jobs

from preview_client.model import (LIMIT, BUNDLE, MANIFEST, INSTALLER, client_asset,
                                  platform_assets, payload_assets, public_assets,
                                  identity_matches, validate_manifest)


def require_job(current: dict, name: str) -> dict:
    job = run_jobs(current['id'], current['run_attempt']).get(name)
    if not job or job['conclusion'] != 'success' or job['head_sha'] != current['head_sha']:
        raise ReleaseError(f'Required preview job has not succeeded: {name}')
    return job


def download_evidence(current: dict, job_name: str, artifact_prefix: str, expected: set[str], directory: Path, *, limit: int = LIMIT) -> int:
    job = require_job(current, job_name)
    attempt = job['evidence_attempt']
    bounds = {name: LIMIT if name.endswith('.tar.xz') else 2 * 1024 * 1024 for name in expected}
    files = artifact_files(current, f'{artifact_prefix}-{attempt}', limit=limit, expected_files=bounds)
    if set(files) != expected:
        raise ReleaseError(f'Unexpected evidence files from {job_name}.')
    directory.mkdir(parents=True, exist_ok=True)
    for name, data in files.items():
        if Path(name).name != name or (directory / name).exists():
            raise ReleaseError('Artifact member path or destination conflicts.')
        (directory / name).write_bytes(data)
    return attempt


def verify_payloads(directory: Path, identity: dict):
    quality = json.loads((directory / 'quality.json').read_text())
    identity_matches(quality, identity)
    package = quality.get('package', {})
    if (quality.get('result') != 'passed' or package.get('source') != identity['snapshot']
            or package.get('dirty', True) or package.get('version') != identity['version']):
        raise ReleaseError('Source quality/package evidence is invalid.')
    for target in identity['targets']:
        archive = f'fluxcope-{target}.tar.xz'
        checksum = digest(directory / archive)
        if (directory / (archive + '.sha256')).read_text().split()[0] != checksum:
            raise ReleaseError('Archive checksum does not match its cargo-dist sidecar.')
        reports = []
        for suffix in ('build-smoke', 'smoke'):
            report = json.loads((directory / f'{target}-{suffix}.json').read_text())
            if (report.get('schema') != 1 or report.get('result') != 'passed' or report.get('dirty', True)
                    or report.get('version') != identity['version'] or report.get('target') != target
                    or report.get('source') != identity['snapshot'] or report.get('archive') != archive
                    or report.get('archive_sha256') != checksum
                    or not re.fullmatch('[0-9a-f]{64}', report.get('binary_sha256', ''))):
                raise ReleaseError('Native preview verification evidence is invalid.')
            reports.append(report)
        if reports[0]['binary_sha256'] != reports[1]['binary_sha256']:
            raise ReleaseError('Build and fresh verifier inspected different executables.')
        native = json.loads((directory / f'{target}-dist-manifest.json').read_text())
        expected = {name: {'sha256': digest(directory / name)} for name in (archive, archive + '.sha256')}
        if (native.get('announcement_tag') != identity['tag'] or native.get('target') != target
                or native.get('artifacts') != expected or native.get('dist_version') != CONFIG['tools']['cargo-dist']):
            raise ReleaseError('Sanitized cargo-dist manifest is invalid.')


def assemble(current: dict, identity: dict, directory: Path):
    if directory.exists() and any(directory.iterdir()):
        raise ReleaseError('Use a fresh publication directory.')
    attempts = {'gate': require_job(current, 'Preview gate')['evidence_attempt']}
    attempts['quality'] = download_evidence(current, 'Preview quality', 'preview-quality', {'quality.json'}, directory)
    for target in identity['targets']:
        attempts[f'build:{target}'] = require_job(current, f'Preview build ({target})')['evidence_attempt']
        attempts[f'verify:{target}'] = download_evidence(current, f'Preview verify ({target})',
                                                      f'preview-verified-{target}', platform_assets(target), directory)
    verify_payloads(directory, identity)
    for target in identity['targets']:
        attempts[f'client:{target}'] = download_evidence(current, f'Preview client ({target})',
            f'preview-client-{target}', {client_asset(target)}, directory)
    template = (Path(__file__).parent / 'preview-installer.sh.in').read_text()
    cases = '\n'.join(f'        {target}) checksum={digest(directory / client_asset(target))} ;;'
                      for target in identity['targets'])
    (directory / INSTALLER).write_text(template.replace('@ID@', str(identity['id'])).replace('@TARGET_CASES@', cases))
    publication = {**identity, 'format': 2}
    manifest = {**publication, 'attempts': attempts, 'toolchain': CONFIG['tools'],
                'files': {name: digest(directory / name) for name in sorted(payload_assets(publication))},
                'result': 'passed', 'local_execution': 'isolated-home-and-port'}
    (directory / MANIFEST).write_text(json.dumps(manifest, sort_keys=True, indent=2) + '\n')


def provenance(path: Path, identity: dict, *, bundle: Path | None = None):
    args = ['gh', 'attestation', 'verify', str(path), '--repo', REPO, '--signer-workflow', SIGNER,
            '--source-ref', f'refs/heads/{BASE}', '--source-digest', identity['controller'], '--deny-self-hosted-runners']
    if bundle is not None:
        args += ['--bundle', str(bundle)]
    run(*args)


def validate_public_directory(directory: Path, identity: dict) -> dict[str, str]:
    manifest = json.loads((directory / MANIFEST).read_text())
    validate_manifest(manifest, identity)
    paths = list(directory.iterdir())
    if ({p.name for p in paths} != public_assets(manifest)
            or any(p.is_symlink() or not p.is_file() or p.stat().st_size > LIMIT for p in paths)):
        raise ReleaseError('Preview assets differ from the publication allowlist.')
    sums = checksums((directory / 'sha256.sum').read_text())
    if set(sums) != public_assets(manifest) - {'sha256.sum'}:
        raise ReleaseError('Preview checksums do not cover exactly the public assets.')
    if any(digest(directory / name) != checksum for name, checksum in sums.items()):
        raise ReleaseError('Preview asset checksum mismatch.')
    if any(sums[name] != checksum for name, checksum in manifest['files'].items()):
        raise ReleaseError('Preview payload differs from the signed manifest.')
    verify_payloads(directory, {**identity, **{key: manifest[key] for key in ('snapshot', 'tree', 'overlay')}})
    for name in [MANIFEST, *(f'fluxcope-{t}.tar.xz' for t in identity['targets'])]:
        provenance(directory / name, identity, bundle=directory / BUNDLE)
    return {**sums, 'sha256.sum': digest(directory / 'sha256.sum')}


def release_for(identity: dict, *, include_drafts: bool = False, token: str | None = None) -> dict | None:
    release = optional_api(f"releases/tags/{identity['tag']}")
    if release is not None or not include_drafts:
        return release
    # GitHub's by-tag endpoint only returns published releases. Draft access
    # requires a write-capable token, supplied only by trusted publication/cleanup.
    matches = [item for item in pages(repository_path('releases?per_page=100'), token=token)
               if item['tag_name'] == identity['tag']]
    if len(matches) > 1:
        raise ReleaseError('Multiple releases claim the same preview tag.')
    return matches[0] if matches else None


def resolve_preview_tag(identity: dict) -> str | None:
    ref = optional_api(f"git/ref/tags/{identity['tag']}")
    if ref is None:
        return None
    if ref['object']['type'] != 'commit':
        raise ReleaseError('Preview tag must point directly to its snapshot commit.')
    return ref['object']['sha']


def download_asset(identity: dict, asset: dict, directory: Path):
    name = asset['name']
    limit = LIMIT if name.endswith('.tar.xz') else 2 * 1024 * 1024
    if name not in public_assets(identity) or asset['size'] > limit:
        raise ReleaseError('Unrecognized or oversized public preview asset.')
    path = directory / name
    if path.exists() or path.is_symlink():
        raise ReleaseError('Download destination must be fresh.')
    url = f"https://github.com/{REPO}/releases/download/{identity['tag']}/{name}"
    # Public accessibility is part of completion. Never attach credentials.
    with urlopen(url, timeout=60) as response, path.open('xb') as stream:
        size = 0
        while chunk := response.read(1024 * 1024):
            size += len(chunk)
            if size > limit:
                raise ReleaseError('Public preview download exceeded its size limit.')
            stream.write(chunk)
    if size != asset['size'] or asset.get('digest') != 'sha256:' + digest(path):
        raise ReleaseError('Public preview asset differs from its GitHub digest.')


def download_public(identity: dict, directory: Path, *, target: str | None = None,
                    metadata_only: bool = False, allow_expired: bool = False) -> tuple[dict, dict]:
    release = release_for(identity)
    if not release or release['draft'] or not release.get('immutable') or not release['prerelease']:
        raise ReleaseError('Preview has no immutable public prerelease.')
    if not allow_expired and datetime.now(timezone.utc) >= timestamp(release['published_at']) + timedelta(days=identity['retention_days']):
        raise ReleaseError('Preview downloads expired. Request a new preview; this ID is never reused.')
    assets = {a['name']: a for a in release['assets']}
    if not {MANIFEST, BUNDLE} <= assets.keys() or len(assets) != len(release['assets']):
        raise ReleaseError('Public preview asset set is invalid.')
    directory.mkdir(parents=True, exist_ok=True)
    for name in (MANIFEST, BUNDLE):
        download_asset(identity, assets[name], directory)
    provenance(directory / MANIFEST, identity, bundle=directory / BUNDLE)
    manifest = json.loads((directory / MANIFEST).read_text())
    validate_manifest(manifest, identity)
    if set(assets) != public_assets(manifest):
        raise ReleaseError('Public preview asset set differs from signed format.')
    if resolve_preview_tag(identity) != manifest['snapshot']:
        raise ReleaseError('Protected preview tag does not match signed source evidence.')
    commit = api(repository_path(f"git/commits/{manifest['snapshot']}"))
    if [p['sha'] for p in commit['parents']] != [identity['source']] or commit['tree']['sha'] != manifest['tree']:
        raise ReleaseError('Preview snapshot ancestry/tree does not match its manifest.')
    if any(assets[name].get('digest') != 'sha256:' + checksum for name, checksum in manifest['files'].items()):
        raise ReleaseError('Public payload digests differ from the signed manifest.')
    if metadata_only:
        return release, manifest
    if target is not None:
        if target not in identity['targets']:
            raise ReleaseError('This preview does not include the host platform; request its profile or all.')
        archive = f'fluxcope-{target}.tar.xz'
        download_asset(manifest, assets[archive], directory)
        provenance(directory / archive, identity, bundle=directory / BUNDLE)
    else:
        for name in sorted(set(assets) - {MANIFEST, BUNDLE}):
            download_asset(manifest, assets[name], directory)
        validate_public_directory(directory, identity)
    return release, manifest
