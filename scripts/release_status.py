#!/usr/bin/env python3
"""Observe exact release runs and public facts; never execute upstream artifact code."""
from __future__ import annotations
import argparse
import base64
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
from urllib.parse import quote

from release_evidence import EvidenceUnavailable, artifact_files, source_package
from release_gate import SourceChecksPending, merged_release, optional_api, registry_version, resolve_tag
from release_publish import expected_assets
from release_support import BASE, CONFIG, REPO, ROOT, ReleaseError, api, eligible_pr, file_at, output, pages, pr_intent, repository_path, run_jobs, version

WORKFLOWS = ('release-plz.yml', 'release.yml', 'publish-homebrew.yml', 'release-recovery.yml')


def authorized_version(value: str, number: int | None = None) -> dict:
    candidates = ([api(repository_path(f'pulls/{number}'))] if number else
                  pages(repository_path(f'pulls?state=closed&base={BASE}&per_page=100')))
    matches = [p for p in candidates if p['merged_at'] and eligible_pr(p) and pr_intent(p)['version'] == value]
    if len(matches) != 1:
        raise ReleaseError('Version must identify one merged release PR.')
    return merged_release(matches[0]['merge_commit_sha'], number=matches[0]['number'], expected=value)


def exact_runs(identity: dict) -> list[dict]:
    result = []
    for filename in WORKFLOWS:
        workflow = api(repository_path(f'actions/workflows/{filename}'))
        query = '' if filename == 'release-recovery.yml' else f"&head_sha={identity['source']}"
        runs = api(repository_path(f"actions/workflows/{workflow['id']}/runs?per_page=100{query}"))['workflow_runs']
        for run in runs:
            if run['workflow_id'] != workflow['id'] or run['repository']['full_name'] != REPO:
                continue
            if filename == 'release-plz.yml':
                match = run['event'] == 'push' and run['head_branch'] == BASE and run['head_sha'] == identity['source']
            elif filename == 'release-recovery.yml':
                # Recovery is dispatched on trusted master, whose SHA may have
                # advanced. The strict run name identifies its requested version;
                # producers still repeat the exact-source authorization gates.
                match = (run['event'] == 'workflow_dispatch' and run['head_branch'] == BASE
                         and run['display_title'] == f"Recover {identity['version']}")
            else:
                match = (run['event'] == 'workflow_dispatch' and run['head_branch'] == f"v{identity['version']}"
                         and run['head_sha'] == identity['source'])
            if match:
                result.append({**run, 'file': filename})
    return result


def homebrew_evidence(identity: dict, runs: list[dict]) -> dict | None:
    completed = sorted((r for r in runs if r['file'] in ('release.yml', 'publish-homebrew.yml')
                        and r['status'] == 'completed'), key=lambda r: r['id'], reverse=True)
    for run in completed:
        update_job = successful_job(run, 'update')
        if not update_job:
            continue
        attempt = update_job['evidence_attempt']
        try:
            commit_files = artifact_files(run, f'tap-commit-{attempt}', limit=1024 * 1024)
            platform_files = {}
            for platform in CONFIG['platforms']:
                target = platform['target']
                job = successful_job(run, f'Homebrew ({target})')
                if not job:
                    raise EvidenceUnavailable(f'Homebrew {target} has not passed.')
                platform_files[target] = artifact_files(run, f"homebrew-{target}-{job['evidence_attempt']}", limit=1024 * 1024)
        except EvidenceUnavailable:
            continue
        if set(commit_files) != {'tap-commit.json'}:
            raise ReleaseError('Invalid tap commit evidence shape.')
        tap = json.loads(commit_files['tap-commit.json'])
        if (tap.get('schema') != 1 or tap.get('source') != identity['source'] or tap.get('version') != identity['version']
                or tap.get('tap') != CONFIG['tap']):
            raise ReleaseError('Tap evidence identity mismatch.')
        formula = api(f"repos/{CONFIG['tap']}/contents/Formula/fluxcope.rb?ref={tap['commit']}")
        if formula.get('encoding') != 'base64' or hashlib.sha256(base64.b64decode(formula['content'])).hexdigest() != tap['formula_sha256']:
            raise ReleaseError('Recorded tap commit does not contain the tested formula.')
        for platform in CONFIG['platforms']:
            target = platform['target']
            files = platform_files[target]
            if set(files) != {f'homebrew-{target}.json'}:
                raise ReleaseError('Invalid Homebrew evidence shape.')
            data = json.loads(files[f'homebrew-{target}.json'])
            if (data.get('schema') != 1 or data.get('source') != identity['source'] or data.get('version') != identity['version']
                    or data.get('result') != 'passed' or data.get('target') != target
                    or data.get('tap_commit') != tap['commit'] or data.get('formula_sha256') != tap['formula_sha256']):
                raise ReleaseError('Homebrew installation evidence does not match the exact tap commit.')
        return {**tap, 'run': run['html_url'], 'attempt': attempt}
    return None


def registry_install_evidence(identity: dict, runs: list[dict]) -> bool:
    for run in sorted(runs, key=lambda r: r['id'], reverse=True):
        if run['file'] not in ('release.yml', 'publish-homebrew.yml'):
            continue
        job = successful_job(run, 'Registry installation')
        if not job:
            continue
        try:
            files = artifact_files(run, f"registry-install-{job['evidence_attempt']}", limit=1024 * 1024)
        except EvidenceUnavailable:
            continue
        if set(files) != {'registry-install.json'}:
            raise ReleaseError('Invalid registry installation evidence shape.')
        data = json.loads(files['registry-install.json'])
        if (data.get('schema') != 1 or data.get('source') != identity['source']
                or data.get('version') != identity['version'] or data.get('result') != 'passed'):
            raise ReleaseError('Registry installation evidence identity mismatch.')
        return True
    return False


def successful_job(run: dict, name: str) -> dict | None:
    jobs = run_jobs(run['id'], run['run_attempt']).values()
    matches = [j for j in jobs if (j['name'] == name or j['name'].endswith(' / ' + name)) and j['head_sha'] == run['head_sha']]
    return matches[0] if len(matches) == 1 and matches[0]['conclusion'] == 'success' else None


def public_evidence(identity: dict, runs: list[dict], release: dict) -> bool:
    for run in sorted(runs, key=lambda r: r['id'], reverse=True):
        if run['file'] != 'release.yml':
            continue
        producer = successful_job(run, 'verify') or successful_job(run, 'publish')
        if not producer:
            continue
        try:
            files = artifact_files(run, f"public-verification-{producer['evidence_attempt']}", limit=1024 * 1024)
        except EvidenceUnavailable:
            continue
        if set(files) != {'public-verification.json'}:
            raise ReleaseError('Invalid public verification evidence shape.')
        report = json.loads(files['public-verification.json'])
        if (report.get('schema') != 1 or report.get('result') != 'passed' or report.get('source') != identity['source']
                or report.get('version') != identity['version'] or report.get('signer') != f'{REPO}/.github/workflows/release.yml'
                or report.get('assets') != {a['name']: a['digest'] for a in release['assets']}):
            raise ReleaseError('Public verification evidence disagrees with immutable assets.')
        return True
    return False


def classify(*, stable: bool, crate: bool, github: bool, tap: bool, installed: bool, running: bool, bootstrap: bool) -> str:
    if github and (not stable or (crate and tap and installed)):
        return 'complete' if stable else 'rc-complete'
    if running:
        return 'running'
    if crate or github:
        return 'partial'
    if stable and bootstrap:
        return 'bootstrap-required'
    return 'failed'


def snapshot(identity: dict) -> dict:
    runs = exact_runs(identity)
    active = [r for r in runs if r['status'] != 'completed']
    try:
        package, _ = source_package(identity)
    except (SourceChecksPending, EvidenceUnavailable):
        if active:
            return {**identity, 'schema': 1, 'state': 'running', 'registry': 'pending', 'github': 'pending',
                    'runs': [{'id': r['id'], 'attempt': r['run_attempt'], 'workflow': r['file'], 'url': r['html_url'],
                              'status': r['status'], 'conclusion': r['conclusion']} for r in runs]}
        raise
    crate = registry_version(identity['version']) if identity['channel'] == 'stable' else None
    if crate and crate['checksum'] != package['sha256']:
        raise ReleaseError('Registry checksum differs from the authorized source package.')
    tag = resolve_tag(identity['version'])
    if tag is not None and tag != identity['source']:
        raise ReleaseError('Tag integrity conflict.')
    release = optional_api(f"releases/tags/v{identity['version']}")
    github = bool(release and not release['draft'])
    if github and (not release.get('immutable') or tag != identity['source']
                   or release['prerelease'] != (identity['channel'] == 'rc')
                   or {a['name'] for a in release['assets']} != expected_assets()
                   or any(not a.get('digest', '').startswith('sha256:') for a in release['assets'])):
        raise ReleaseError('Published GitHub release identity, immutability or assets are invalid.')
    # Only the verified distribution workflow may attest that public download,
    # provenance and installation checks completed. Asset existence alone is insufficient.
    distribution_ok = public_evidence(identity, runs, release) if github else False
    tap = homebrew_evidence(identity, runs) if github and identity['channel'] == 'stable' else None
    installed = registry_install_evidence(identity, runs) if github and identity['channel'] == 'stable' else False
    archived = None
    archive_error = None
    if github:
        from result_archive import read as read_archive
        try:
            archived = read_archive(identity, release, package)
        except (ReleaseError, ValueError, KeyError) as error:
            # Reject the archive as evidence. Independent live checks may still
            # authorize re-signing; otherwise recovery must rerun missing checks.
            archive_error = f'Archived result was not accepted: {error}'
        if archived:
            distribution_ok = True
            if identity['channel'] == 'stable':
                tap = tap or archived['homebrew']
                installed = installed or archived['registry_install']
    bootstrap = (os.environ.get('CRATES_BOOTSTRAPPED') == 'false'
                 and any(r['file'] == 'release-plz.yml' and r['conclusion'] == 'success' for r in runs))
    state = classify(stable=identity['channel'] == 'stable', crate=bool(crate), github=github and distribution_ok,
                     tap=bool(tap), installed=installed, running=bool(active), bootstrap=bootstrap)
    if github and state == 'failed':
        state = 'partial'
    return {**identity, 'schema': 1, 'state': state, 'package': package, 'public_verified': distribution_ok,
            'registry': 'verified' if crate else ('not-applicable' if identity['channel'] == 'rc' else 'pending'),
            'github': release['html_url'] if github else 'pending', 'homebrew': tap or ('not-applicable' if identity['channel'] == 'rc' else 'pending'),
            'registry_install': installed if identity['channel'] == 'stable' else 'not-applicable',
            'archived': bool(archived),
            'error': archive_error,
            'runs': [{'id': r['id'], 'attempt': r['run_attempt'], 'workflow': r['file'], 'url': r['html_url'],
                      'status': r['status'], 'conclusion': r['conclusion']} for r in runs]}


def write_check(identity: dict, report: dict):
    state = report['state']
    summary = (f"Version **{identity['version']}** · `{identity['source']}` · [PR #{identity['pr']}]({identity['url']})\n\n"
               f"State: **{state}**\n\nRegistry: {report.get('registry', 'unverified')}\n\nGitHub: {report.get('github', 'unverified')}\n\n")
    if report.get('error'):
        summary += f"Observation failed: {report['error']}\n\n"
    if isinstance(report.get('homebrew'), dict):
        summary += f"Tap: `{report['homebrew']['commit']}` — all four installation checks passed.\n\n"
    for run in report.get('runs', []):
        summary += f"- [{run['workflow']} run {run['id']} attempt {run['attempt']}]({run['url']}): {run['conclusion'] or run['status']}\n"
    summary += f"\nResume: `just release-status {identity['version']}` · Recovery: `just release-recover {identity['version']}`\n"
    url = f"https://github.com/{REPO}/actions/runs/{os.environ['GITHUB_RUN_ID']}"
    name = f"Release {identity['version']}"
    existing = api(repository_path(f"commits/{identity['source']}/check-runs?check_name={quote(name)}&per_page=100"))['check_runs']
    owned = [c for c in existing if c['name'] == name and c['head_sha'] == identity['source'] and c['app']['slug'] == 'github-actions']
    check = max(owned, key=lambda c: c['id'], default=None)
    payload = {'name': name, 'details_url': url, 'external_id': f"fluxcope:{identity['source']}:{identity['version']}",
               'status': 'in_progress' if state in ('preparing', 'running') else 'completed',
               'output': {'title': f"{identity['version']}: {state}", 'summary': summary[:60000]}}
    if payload['status'] == 'completed':
        payload['conclusion'] = 'success' if state in ('complete', 'rc-complete') else 'action_required'
        payload['completed_at'] = datetime.now(timezone.utc).isoformat()
    if check:
        api(repository_path(f"check-runs/{check['id']}"), method='PATCH', payload=payload)
    else:
        api(repository_path('check-runs'), method='POST', payload={**payload, 'head_sha': identity['source']})
    (ROOT / 'target').mkdir(exist_ok=True)
    (ROOT / 'target/release-result.json').write_text(json.dumps(report, indent=2) + '\n')
    Path(os.environ['GITHUB_STEP_SUMMARY']).write_text(summary)


def event_identity(event: dict) -> dict | None:
    hinted = event['workflow_run']
    run = api(repository_path(f"actions/runs/{hinted['id']}"))
    ids = {api(repository_path(f'actions/workflows/{name}'))['id']: name for name in WORKFLOWS}
    if (run['workflow_id'] not in ids or run['repository']['full_name'] != REPO
            or run['run_attempt'] != hinted['run_attempt'] or run['event'] not in ('push', 'workflow_dispatch')):
        return None
    name = ids[run['workflow_id']]
    if name == 'release-plz.yml':
        if run['event'] != 'push' or run['head_branch'] != BASE:
            return None
        associated = api(repository_path(f"commits/{run['head_sha']}/pulls?per_page=100"))
        if not any(p['merged_at'] and p['merge_commit_sha'] == run['head_sha'] and eligible_pr(p) for p in associated):
            return None
        return merged_release(run['head_sha'])
    if name == 'release-recovery.yml':
        if run['head_branch'] != BASE or not run['display_title'].startswith('Recover '):
            return None
        return authorized_version(version(run['display_title'].removeprefix('Recover ')))
    if not run['head_branch'].startswith('v'):
        return None
    return merged_release(run['head_sha'], expected=version(run['head_branch'][1:]))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--version', default='')
    parser.add_argument('--archive', choices=('prepare', 'publish'))
    parser.add_argument('--pr', type=int)
    args = parser.parse_args()
    if os.environ.get('GITHUB_REPOSITORY') != REPO or os.environ.get('GITHUB_REF') != f'refs/heads/{BASE}':
        raise ReleaseError('Observer code must run from the trusted default branch.')
    if args.archive:
        identity = authorized_version(version(args.version), args.pr)
        from result_archive import encode, prepare as prepare_archive, write as archive_result
        path = ROOT / 'target/release-archive.json'
        try:
            if args.archive == 'prepare':
                report = snapshot(identity)
                report['observer_run'] = os.environ['GITHUB_RUN_ID']
                release = api(repository_path(f"releases/tags/v{identity['version']}"))
                saved = prepare_archive(identity, report, release)
                path.parent.mkdir(exist_ok=True)
                path.write_bytes(encode(saved))
            else:
                saved = json.loads(path.read_text())
                if saved.get('observer_run') != os.environ['GITHUB_RUN_ID']:
                    raise ReleaseError('Archive input must come from this observer run.')
                archive_result(identity, saved)
                report = snapshot(identity)
                if not report.get('archived'):
                    raise ReleaseError('Signed durable result was not confirmed.')
                write_check(identity, report)
        except Exception as error:
            write_check(identity, {**identity, 'schema': 1, 'state': 'partial', 'error': f'Result archival failed: {error}'})
            raise
    else:
        identity = (authorized_version(version(args.version), args.pr) if args.version else
                    event_identity(json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())))
        if identity:
            try:
                report = snapshot(identity)
            except Exception as error:
                report = {**identity, 'schema': 1, 'state': 'failed', 'error': str(error)}
            report['observer_run'] = os.environ['GITHUB_RUN_ID']
            needs_archive = report['state'] in ('complete', 'rc-complete') and not report.get('archived')
            output('archive', 'true' if needs_archive else 'false')
            output('version', identity['version'])
            output('pr', identity['pr'])
            if needs_archive:
                # A completed action-required check cannot remain stuck running
                # if checkout, signing or environment startup fails afterwards.
                write_check(identity, {**report, 'state': 'partial', 'archive_pending': True,
                                       'error': 'All channels verified; waiting for signed result archival.'})
                (ROOT / 'target/release-result.json').write_text(json.dumps(report, indent=2) + '\n')
            else:
                write_check(identity, report)
