"""Read bounded data from an exact successful CI run; artifacts never supply code."""
from __future__ import annotations
import hashlib
import json
from pathlib import Path
import subprocess
import struct
import tempfile
import zipfile
from urllib.request import urlopen

from release_gate import optional_api, resolve_tag, source_checks
from release_support import CONFIG, REPO, ReleaseError, api, repository_path, run_jobs


class EvidenceUnavailable(ReleaseError):
    """Expected evidence is absent or expired; not an API/authentication failure."""


def artifact_files(run: dict, name: str, *, limit: int = 32 * 1024 * 1024,
                   expected_files: dict[str, int] | None = None) -> dict[str, bytes]:
    artifacts = api(repository_path(f"actions/runs/{run['id']}/artifacts?per_page=100"))['artifacts']
    matches = [a for a in artifacts if a['name'] == name and not a['expired']]
    if not matches:
        raise EvidenceUnavailable(f'Missing or expired artifact {name} in run {run["id"]}.')
    if len(matches) != 1:
        raise ReleaseError(f'Ambiguous artifact {name} in run {run["id"]}.')
    artifact = matches[0]
    if artifact['size_in_bytes'] > limit or artifact['workflow_run']['head_sha'] != run['head_sha']:
        raise ReleaseError('Artifact size or source is invalid.')
    with tempfile.TemporaryFile() as downloaded:
        result = subprocess.run(['gh', 'api', repository_path(f"actions/artifacts/{artifact['id']}/zip")],
                                stdout=downloaded, stderr=subprocess.PIPE, check=False)
        if result.returncode:
            raise ReleaseError('Could not download verified CI artifact: ' + result.stderr.decode(errors='replace'))
        if downloaded.tell() > limit:
            raise ReleaseError('Downloaded artifact exceeds its size limit.')
        # ZipFile materializes the central directory at construction. Bound its
        # entry count first, including empty members, and reject unnecessary ZIP64.
        downloaded.seek(max(0, downloaded.tell() - 65557))
        tail = downloaded.read()
        end = tail.rfind(b'PK\x05\x06')
        if end < 0 or len(tail) - end < 22:
            raise ReleaseError('Artifact has no valid ZIP directory.')
        header = struct.unpack('<4s4H2LH', tail[end:end + 22])
        max_members = len(expected_files) + 16 if expected_files is not None else 1024
        if (header[4] > max_members or header[1] or header[2]
                or header[5] == 0xffffffff or header[6] == 0xffffffff
                or tail[max(0, end - 20):end - 16] == b'PK\x06\x07'):
            raise ReleaseError('Artifact ZIP directory exceeds its member/format bounds.')
        downloaded.seek(0)
        with zipfile.ZipFile(downloaded) as archive:
            members = archive.infolist()
            names = [m.filename for m in members if not m.is_dir()]
            if len(names) != len(set(names)) or sum(m.file_size for m in members) > limit:
                raise ReleaseError('Duplicate artifact member or excessive uncompressed size.')
            if any(Path(n).is_absolute() or '..' in Path(n).parts for n in names):
                raise ReleaseError('Artifact includes an unsafe path.')
            if expected_files is not None and (set(names) != set(expected_files)
                    or any(m.file_size > expected_files[m.filename] for m in members if not m.is_dir())):
                raise ReleaseError('Artifact members differ from the expected names or per-file size limits.')
            return {n: archive.read(n) for n in names}


def source_package(identity: dict, *, wait: bool = False) -> tuple[dict, bytes]:
    run_id = source_checks(identity['source'], wait=wait)
    run = api(repository_path(f'actions/runs/{run_id}'))
    if run['repository']['full_name'] != REPO or run['head_sha'] != identity['source']:
        raise ReleaseError('CI artifact run identity changed.')
    package_job = run_jobs(run_id, run['run_attempt'])['Package']
    package_attempt = package_job['evidence_attempt']
    try:
        files = artifact_files(run, f'cargo-package-{package_attempt}')
    except EvidenceUnavailable:
        # Immutable public source metadata outlives Actions artifact retention.
        # An unpublished bootstrap still requires the original CI package; rerun
        # that exact CI commit if its package artifact has expired.
        release = optional_api(f"releases/tags/v{identity['version']}")
        if not release or release['draft'] or not release.get('immutable') or resolve_tag(identity['version']) != identity['source']:
            raise
        assets = [a for a in release['assets'] if a['name'] == 'source-evidence.json']
        if len(assets) != 1 or assets[0]['size'] > 1024 * 1024:
            raise ReleaseError('Immutable source evidence is missing or invalid.')
        url = f"https://github.com/{REPO}/releases/download/v{identity['version']}/source-evidence.json"
        with urlopen(url, timeout=30) as stream:
            data = stream.read(1024 * 1024 + 1)
        if len(data) > 1024 * 1024 or assets[0].get('digest') != 'sha256:' + hashlib.sha256(data).hexdigest():
            raise ReleaseError('Immutable source evidence digest conflict.')
        evidence = json.loads(data)
        report = evidence.get('package', {})
        if (evidence.get('schema') != 1 or evidence.get('source') != identity['source'] or evidence.get('version') != identity['version']
                or report.get('schema') != 1 or report.get('source') != identity['source'] or report.get('version') != identity['version']
                or report.get('dirty', True) or report.get('ci_run') != run_id):
            raise ReleaseError('Immutable source evidence identity conflict.')
        return report, b''
    reports = [data for name, data in files.items() if Path(name).name == 'package-report.json']
    crates = [data for name, data in files.items() if Path(name).name == f"{CONFIG['crate']}-{identity['version']}.crate"]
    if len(reports) != 1 or len(crates) != 1 or len(files) != 2:
        raise ReleaseError('Unexpected source package artifact contents.')
    report = json.loads(reports[0])
    if (report.get('schema') != 1 or report.get('dirty', True)
            or report.get('source') != identity['source'] or report.get('version') != identity['version']
            or report.get('crate') != CONFIG['crate'] or report.get('sha256') != hashlib.sha256(crates[0]).hexdigest()):
        raise ReleaseError('Source package evidence does not match the authorized clean source.')
    return {**report, 'ci_run': run_id, 'ci_attempt': run['run_attempt'], 'package_attempt': package_attempt}, crates[0]
