"""Wait for the complete required-check set before interactive merge approval."""
import json
import time
from urllib.parse import quote

from release_support import REPO, ReleaseError, api, pages, repository_path, run, safe_text


def required_checks(base: str) -> set[str]:
    branch = quote(base, safe='')
    rules = pages(repository_path(f'rules/branches/{branch}?per_page=100'))
    names = {check['context'] for rule in rules if rule['type'] == 'required_status_checks'
             for check in rule['parameters']['required_status_checks']}
    # Read classic protection through GraphQL; the REST protection endpoint
    # would introduce an unnecessary Administration:read token requirement.
    query = '''query($owner: String!, $repo: String!, $ref: String!) {
      repository(owner: $owner, name: $repo) {
        ref(qualifiedName: $ref) {
          branchProtectionRule { requiresStatusChecks requiredStatusCheckContexts }
        }
      }
    }'''
    owner, repository = REPO.split('/')
    data = json.loads(run('gh', 'api', 'graphql', '-f', f'query={query}', '-f', f'owner={owner}',
                          '-f', f'repo={repository}', '-f', f'ref=refs/heads/{base}'))
    reference = data['data']['repository']['ref']
    if reference is None:
        raise ReleaseError(f'Cannot read required checks: base branch {base} is missing.')
    protection = reference['branchProtectionRule']
    if protection and protection['requiresStatusChecks']:
        names.update(protection['requiredStatusCheckContexts'] or [])
    return names


def reported_checks(number: int) -> list[dict]:
    try:
        # gh handles pagination and GitHub's isRequired selection. JSON mode
        # returns check states as data, including failures and pending jobs.
        return json.loads(run('gh', 'pr', 'checks', str(number), '--repo', REPO,
                              '--required', '--json', 'name,state,bucket,link'))
    except ReleaseError as error:
        # The CLI reports this before the first check has registered. Do not
        # mistake authentication, API or network failures for an empty list.
        if str(error).startswith(('gh pr failed (1): no checks reported on the ',
                                  'gh pr failed (1): no required checks reported on the ')):
            return []
        raise


def revision(pr: dict) -> tuple[str, str, str]:
    return pr['head']['sha'], pr['base']['sha'], pr['base']['ref']


def wait_for_merge(pr: dict, *, resume: str, timeout: float = 90 * 60) -> dict:
    """Return a mergeable, merged, or changed PR; never mutate remote state."""
    number, url = pr['number'], pr['html_url']
    started = time.monotonic()
    deadline = started + timeout
    settling_since = None
    previous = None
    last_printed = started
    try:
        while True:
            current = api(repository_path(f'pulls/{number}'))
            if current['merged'] or revision(current) != revision(pr):
                return current
            if current['state'] != 'open':
                raise ReleaseError(f'PR #{number} is closed without merging. Inspect {url}.')
            if current['draft']:
                raise ReleaseError(f'PR #{number} is a draft. Mark it ready with '
                                   f'gh pr ready {number} --repo {REPO}, then {resume}.')
            expected = required_checks(current['base']['ref'])
            checks = reported_checks(number)
            # A push/base update during API requests invalidates their result.
            current = api(repository_path(f'pulls/{number}'))
            if current['merged'] or revision(current) != revision(pr):
                return current
            if current['state'] != 'open' or current['draft']:
                continue
            state = current['mergeable_state']
            if state == 'behind':
                raise ReleaseError(f'PR #{number} must be updated with {current["base"]["ref"]}. '
                                   f'Update its branch at {url}, let checks rerun, then {resume}.')
            if state == 'dirty' or current['mergeable'] is False:
                raise ReleaseError(f'PR #{number} has merge conflicts. Resolve them at {url}, '
                                   f'push the resolution, then {resume}.')
            failed = [check for check in checks if check['bucket'] in ('fail', 'cancel')]
            if failed:
                details = '\n'.join(f"- {c['name']}: {c['state']} — {c.get('link') or url + '/checks'}"
                                    for c in failed)
                raise ReleaseError(f'Required checks failed for PR #{number}:\n{safe_text(details)}\n'
                                   f'Fix or rerun those checks, then {resume}.')
            missing = sorted(expected - {check['name'] for check in checks})
            pending = [f"{check['name']} ({check['state'].lower()})" for check in checks
                       if check['bucket'] not in ('pass', 'skipping')]
            waiting = [f'{name} (not reported yet)' for name in missing] + sorted(pending)
            now = time.monotonic()
            if waiting:
                settling_since = None
                message = 'Waiting for required checks: ' + ', '.join(waiting)
            elif state in ('clean', 'has_hooks') and current['mergeable'] is True:
                print(f'PR #{number}: all required checks passed; ready for merge review.', flush=True)
                return current
            else:
                # GitHub can briefly retain BLOCKED/UNKNOWN after checks finish.
                # Give that computation time, but don't hide a persistent rule.
                if settling_since is None:
                    settling_since = now
                review = json.loads(run('gh', 'pr', 'view', str(number), '--repo', REPO,
                                        '--json', 'reviewDecision'))['reviewDecision']
                if review in ('REVIEW_REQUIRED', 'CHANGES_REQUESTED'):
                    raise ReleaseError(f'PR #{number} needs review approval ({review.lower()}). '
                                       f'Resolve the review at {url}, then {resume}.')
                if now - settling_since >= 120:
                    raise ReleaseError(f'All required checks passed, but GitHub still reports PR #{number} '
                                       f'as {state}. Inspect outstanding conversations and branch rules at '
                                       f'{url}, then {resume}.')
                message = f'All required checks passed; waiting for GitHub merge status ({state})'
            if message != previous or now - last_printed >= 60:
                print(safe_text(f'PR #{number}: {message}. Elapsed {int(now - started)}s.\n'
                                f'{url}/checks'), flush=True)
                previous, last_printed = message, now
            if now >= deadline:
                raise ReleaseError(f'Stopped waiting for PR #{number}. Remote checks continue. '
                                   f'Inspect {url}/checks and resume with {resume}.')
            time.sleep(min(10, deadline - now))
    except KeyboardInterrupt:
        raise KeyboardInterrupt(f'Local wait stopped. Remote checks continue; resume with {resume}.') from None
