#!/usr/bin/env python3
"""Interactive maintainer commands. Publishing remains in verified GitHub workflows."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
import shutil
import sys
import tempfile
import time

from release_support import (
    BASE, CONFIG, CONVENTIONAL, REPO, ROOT, ReleaseError, api, dispatch, eligible_pr,
    pages, pr_intent, release_branch, repository_path, run, safe_text,
    verify_local_repository, version,
)


def confirm(message: str) -> None:
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        raise ReleaseError("This operation requires an interactive terminal and explicit confirmation.")
    if input(f"{message} [y/N] ").strip().lower() not in ("y", "yes"):
        raise ReleaseError("Cancelled; no merge or release was authorized.")


def choose(items: list[dict], label) -> dict:
    if len(items) == 1:
        return items[0]
    if not items:
        raise ReleaseError("No matching remote object was found.")
    for index, item in enumerate(items, 1):
        print(f"{index}. {safe_text(label(item))}")
    if not sys.stdin.isatty():
        raise ReleaseError("Multiple candidates require an interactive selection.")
    try:
        selected = int(input("Select a number: "))
        if not 1 <= selected <= len(items):
            raise ValueError
        return items[selected - 1]
    except ValueError as error:
        raise ReleaseError("Invalid selection.") from error


def doctor() -> None:
    problems = []
    for executable, arguments, fix in (
        ("git", ("--version",), "Install Git from https://git-scm.com/downloads"),
        ("just", ("--version",), "brew install just"),
        ("gh", ("--version",), "brew install gh"),
    ):
        if shutil.which(executable):
            print(run(executable, *arguments).splitlines()[0])
        else:
            problems.append(f"Missing {executable}: {fix}")
    print(f"Python {sys.version.split()[0]} (requires 3.11+)")
    if sys.version_info < (3, 11):
        problems.append("Install Python 3.11 or newer.")
    try:
        verify_local_repository()
    except ReleaseError as error:
        problems.append(str(error) + f" Expected origin: git@github.com:{REPO}.git")
    if shutil.which("gh"):
        try:
            # An inactive expired account must not invalidate the active login.
            api("user")
            remote = api(repository_path())
            if not remote.get("permissions", {}).get("push"):
                problems.append(f"The authenticated account needs push permission on {REPO}.")
            workflow_data = api(repository_path("actions/workflows?per_page=100"))
            active = {Path(w["path"]).name for w in workflow_data["workflows"] if w["state"] == "active"}
            for filename in ("ci.yml", "release-plz.yml", "release.yml", "release-status.yml", "release-recovery.yml"):
                if filename not in active:
                    problems.append(f"Workflow missing or inactive on {BASE}: {filename}")
        except ReleaseError as error:
            problems.append(f"GitHub access failed: {error}\nRun: gh auth login --hostname github.com --git-protocol ssh --web")
    if problems:
        raise ReleaseError("\n".join(problems))
    print("Release prerequisites are available. No configuration or authentication was changed.")


def pull_requests(state: str = "open") -> list:
    return pages(repository_path(f"pulls?state={state}&base={BASE}&per_page=100"))


def feature_pr(*, resume: bool) -> dict:
    branch = run("git", "symbolic-ref", "--quiet", "--short", "HEAD")
    if branch == BASE or release_branch(branch):
        raise ReleaseError("Use a feature branch for pr/ship; use just release for an existing release PR.")
    head = run("git", "rev-parse", "HEAD")
    dirty = run("git", "status", "--short", "--untracked-files=all")
    if dirty:
        print("Local changes below will NOT be included or modified:\n" + safe_text(dirty))
    candidates = [p for p in pull_requests("all") if p["head"].get("repo")
                  and p["head"]["repo"]["full_name"] == REPO and p["head"]["ref"] == branch]
    merged = [p for p in candidates if p["merged_at"] and p["head"]["sha"] == head]
    if merged and resume:
        return choose(merged, lambda p: f"#{p['number']} {p['title']} {p['html_url']}")
    run("git", "fetch", "origin", BASE, capture=False)
    print(f"Repository: {REPO}\nBranch: {branch}\nCommitted head: {head}")
    print(safe_text(run("git", "log", "--oneline", f"origin/{BASE}..{head}")))
    if not run("git", "rev-list", f"origin/{BASE}..{head}"):
        raise ReleaseError("There are no feature commits to propose.")
    # Explicit push, no force and no implicit gh fork/push. Git handles the remote lease.
    run("git", "push", "--set-upstream", "origin", f"HEAD:refs/heads/{branch}", capture=False)
    opened = [p for p in candidates if p["state"] == "open"]
    if opened:
        selected = choose(opened, lambda p: f"#{p['number']} {p['title']}")
        return api(repository_path(f"pulls/{selected['number']}"))
    subjects = run("git", "log", "--format=%s", f"origin/{BASE}..{head}").splitlines()
    suggestion = subjects[0] if len(subjects) == 1 and CONVENTIONAL.fullmatch(subjects[0]) else ""
    if not sys.stdin.isatty():
        raise ReleaseError("The branch was pushed. Run just pr interactively to review its PR title.")
    print("Committed changes:\n" + safe_text("\n".join(subjects)))
    title = input(f"Conventional Commit PR title{f' [{suggestion}]' if suggestion else ''}: ").strip() or suggestion
    if not CONVENTIONAL.fullmatch(title):
        raise ReleaseError("Use a Conventional Commit title, e.g. feat: add request mapping.")
    body = "## Changes\n\n" + "\n".join(f"- {subject}" for subject in subjects) + "\n\n## Validation\n\nSee CI checks for the exact PR head.\n"
    with tempfile.TemporaryDirectory(prefix="fluxcope-pr-") as directory:
        path = Path(directory) / "body.md"
        path.write_text(body)
        run("gh", "pr", "create", "--repo", REPO, "--base", BASE, "--head", branch,
            "--title", title, "--body-file", str(path), capture=False)
    matches = [p for p in pull_requests() if p["head"].get("repo")
               and p["head"]["repo"]["full_name"] == REPO and p["head"]["ref"] == branch]
    return choose(matches, lambda p: f"#{p['number']} {p['title']}")


def review_and_merge(pr: dict, *, release: bool) -> dict:
    number = pr["number"]
    while True:
        current = api(repository_path(f"pulls/{number}"))
        if current["merged"]:
            return current
        if current["state"] != "open" or current["draft"]:
            raise ReleaseError(f"PR #{number} must be open and ready for review.")
        head = current["head"]["sha"]
        intent = pr_intent(current) if release else None
        print(f"\nRepository: {REPO}\nPR: {current['html_url']}\nHead: {head}")
        print(safe_text(current["title"]))
        if intent:
            print(f"Release: {intent['version']} ({intent['channel']})")
        print(f"Review all changes: {current['html_url']}/files")
        print(safe_text(current.get("body") or ""))
        run("gh", "pr", "checks", str(number), "--repo", REPO, "--required", "--watch", "--interval", "10", capture=False)
        refreshed = api(repository_path(f"pulls/{number}"))
        if refreshed["head"]["sha"] != head:
            print("PR head changed. Review the refreshed changes and checks.")
            continue
        if refreshed["mergeable_state"] not in ("clean", "has_hooks"):
            raise ReleaseError("PR is not mergeable with current branch rules; resolve the indicated checks or base update.")
        if intent:
            confirm(f"I reviewed this complete release and any required TUI experience check. Merge to publish {intent['version']}?")
        else:
            confirm("I reviewed the feature changes at this head. Squash merge this feature PR?")
        run("gh", "pr", "merge", str(number), "--repo", REPO, "--squash", "--match-head-commit", head, capture=False)
        result = api(repository_path(f"pulls/{number}"))
        if not result["merged"] or not result.get("merge_commit_sha"):
            raise ReleaseError("Merge is not confirmed. Recheck the PR; no release completion is assumed.")
        return result


def release_candidates(contains: str | None = None) -> list:
    result = []
    for pr in pull_requests():
        if not eligible_pr(pr):
            continue
        pr_intent(pr)
        if contains:
            comparison = api(repository_path(f"compare/{contains}...{pr['head']['sha']}"))
            if comparison["status"] not in ("ahead", "identical"):
                continue
        result.append(pr)
    return result


def release_existing(contains: str | None = None) -> None:
    candidates = release_candidates(contains)
    if not candidates:
        raise ReleaseError("No eligible release PR contains the requested source. Run just release-prepare, then just release. A docs-only change may require no release.")
    selected = choose(candidates, lambda p: f"#{p['number']} {p['title']} {p['html_url']}")
    merged = review_and_merge(selected, release=True)
    intent = pr_intent(merged)
    status(intent["version"], source=merged["merge_commit_sha"])


def reports() -> list:
    # The observer writes one check per release source. Query merged release PRs to
    # discover versions; never substitute the local manifest or latest master tip.
    results = []
    for pr in pull_requests("closed"):
        if not pr["merged_at"] or not eligible_pr(pr):
            continue
        intent = pr_intent(pr)
        results.append({"version": intent["version"], "source": pr["merge_commit_sha"], "pr": pr["number"]})
    return results


def read_check(target: dict) -> dict | None:
    data = api(repository_path(f"commits/{target['source']}/check-runs?check_name=Release%20{target['version']}&per_page=100"))
    matches = [c for c in data["check_runs"] if c["name"] == f"Release {target['version']}"
               and c["app"]["slug"] == "github-actions" and c["head_sha"] == target["source"]]
    return max(matches, key=lambda c: c["id"], default=None)


def status(requested: str | None, *, source: str | None = None) -> None:
    candidates = reports()
    if requested:
        candidates = [r for r in candidates if r["version"] == requested and (source is None or r["source"] == source)]
    elif candidates:
        unfinished = [r for r in candidates if (c := read_check(r)) is None or c["conclusion"] != "success"]
        candidates = unfinished or candidates[:1]
    target = choose(candidates, lambda r: f"{r['version']} source {r['source']} PR #{r['pr']}")
    last = None
    deadline = time.monotonic() + 6 * 60 * 60
    while time.monotonic() < deadline:
        check = read_check(target)
        if check:
            snapshot = (check["status"], check["conclusion"], check["output"].get("summary"))
            if snapshot != last:
                print(safe_text(check["output"].get("summary") or check["output"].get("title", "")))
                print(check["details_url"] or check["html_url"])
                last = snapshot
            if check["status"] == "completed":
                if check["conclusion"] == "success":
                    return
                observer = re.fullmatch(rf'https://github.com/{re.escape(REPO)}/actions/runs/([0-9]+)', check.get('details_url') or '')
                if not observer or api(repository_path(f'actions/runs/{observer[1]}'))['status'] == 'completed':
                    raise ReleaseError(f"Release needs attention. Run: just release-recover {target['version']}")
        elif last is None:
            print(f"Waiting for Release {target['version']} on {target['source']}…")
            last = "waiting"
        time.sleep(10)
    raise ReleaseError(f"Stopped waiting. Remote work continues; resume with just release-status {target['version']}.")


def ship() -> None:
    pr = feature_pr(resume=True)
    merged = pr if pr["merged_at"] else review_and_merge(pr, release=False)
    source = merged["merge_commit_sha"]
    # A previous invocation may already have merged the release PR.
    for candidate in reports():
        comparison = api(repository_path(f"compare/{source}...{candidate['source']}"))
        if comparison["status"] in ("ahead", "identical"):
            status(candidate["version"], source=candidate["source"])
            return
    print(f"Feature merged at {source}. Waiting for a release PR containing it…")
    deadline = time.monotonic() + 45 * 60
    refreshed = False
    while time.monotonic() < deadline:
        if release_candidates(source):
            release_existing(source)
            return
        if not refreshed:
            dispatch("release-plz.yml", BASE, {"version": ""})
            refreshed = True
        time.sleep(10)
    raise ReleaseError("Feature merge succeeded. No matching release PR appeared; inspect Prepare release for no package changes, RC intent, or an incomplete earlier release. Continue with just release after resolving it.")


def recover(requested: str) -> None:
    from release_recovery import plan
    from release_status import authorized_version
    matches = [r for r in reports() if r["version"] == requested]
    target = choose(matches, lambda r: f"{r['version']} source {r['source']} PR #{r['pr']}")
    identity = authorized_version(requested, target['pr'])
    bootstrap = api(repository_path('actions/variables/CRATES_BOOTSTRAPPED'))['value']
    if bootstrap not in ('true', 'false'):
        raise ReleaseError('CRATES_BOOTSTRAPPED is missing or invalid; run just doctor.')
    selected = plan(identity, bootstrap=bootstrap == 'false')
    print(safe_text(f"{requested} at {identity['source']}\n{selected['description']}"))
    if selected['operation'] == 'wait':
        print(selected['url'])
        return
    if selected['operation'] == 'bootstrap-upload':
        print(f"Follow docs/releasing.md. Exact reviewed source: {identity['source']}; PR #{identity['pr']}.")
        return
    confirm(f"Execute {selected['operation']} for this authorized version?")
    dispatch("release-recovery.yml", BASE, {"version": requested, "pr": str(target["pr"]), "operation": selected['operation']})
    print(f"Recovery requested. Follow with: just release-status {requested}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("doctor", "pr", "ship", "release", "prepare", "status", "recover", "replace"))
    parser.add_argument("version", nargs="?", default="")
    args = parser.parse_args()
    requested = version(args.version) if args.version else None
    if args.command == "doctor":
        doctor()
        return
    verify_local_repository()
    if args.command == "pr":
        pr = feature_pr(resume=False)
        print(f"PR: {pr['html_url']}\nHead: {pr['head']['sha']}\nChecks: {pr['html_url']}/checks")
    elif args.command == "ship":
        ship()
    elif args.command == "release":
        release_existing()
    elif args.command == "prepare":
        dispatch("release-plz.yml", BASE, {"version": requested or ""})
        print(f"Prepare release requested: https://github.com/{REPO}/actions/workflows/release-plz.yml")
    elif args.command == "status":
        status(requested)
    elif args.command == "replace":
        from release_status import authorized_version
        if not requested:
            raise ReleaseError('Replacement requires an existing authorized version.')
        identity = authorized_version(requested)
        print(f"Classify {requested} ({identity['source']}) as awaiting replacement. Its original result stays incomplete.")
        reason = input('Public reason for preparing a new version: ').strip()
        confirm(f"Record this public classification for {requested}: {safe_text(reason)}?")
        dispatch('release-recovery.yml', BASE, {'version': requested, 'pr': str(identity['pr']),
                 'operation': 'mark-for-replacement', 'reason': reason})
    elif args.command == "recover":
        if not requested:
            raise ReleaseError("Recovery requires a version, e.g. just release-recover 0.1.0.")
        recover(requested)


if __name__ == "__main__":
    try:
        main()
    except (ReleaseError, OSError, ValueError, KeyError) as error:
        print(f"Release: {safe_text(error)}", file=sys.stderr)
        sys.exit(1)
    except KeyboardInterrupt:
        print("\nLocal wait stopped. Remote jobs continue; use just release-status to resume.", file=sys.stderr)
        sys.exit(130)
