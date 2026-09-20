"""Shared release identity and GitHub CLI transport; standard library only."""

from __future__ import annotations

import base64
import json
import os
from pathlib import Path
import re
import subprocess
from functools import lru_cache
from urllib.parse import quote

ROOT = Path(__file__).resolve().parents[1]
CONFIG = json.loads((ROOT / ".github/release-config.json").read_text())
REPO = CONFIG["repository"]
BASE = CONFIG["base"]
VERSION = re.compile(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-rc\.([1-9]\d*))?")
SHA = re.compile(r"[0-9a-f]{40}")
CONVENTIONAL = re.compile(r"(?:feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)(?:\([^\r\n()]+\))?!?: \S.+")


from preview_client.model import PreviewError as ReleaseError


def run(*args: str, cwd: Path = ROOT, capture: bool = True, env: dict | None = None) -> str:
    completed = subprocess.run(args, cwd=cwd, text=True, capture_output=capture, env=env, shell=False)
    if completed.returncode:
        detail = completed.stderr.strip() if capture else "see command output above"
        raise ReleaseError(f"{args[0]} {args[1] if len(args) > 1 else ''} failed ({completed.returncode}): {detail}")
    return completed.stdout.strip() if capture else ""


def api(endpoint: str, *, method: str = "GET", payload: dict | None = None, token: str | None = None) -> object:
    args = ["gh", "api", endpoint, "--method", method,
            "-H", "Accept: application/vnd.github+json", "-H", "X-GitHub-Api-Version: 2022-11-28"]
    environment = None
    # Reads use the workflow token unless a caller explicitly needs another
    # credential (for example, draft releases require Contents write access).
    # The implicit App-token fallback remains restricted to mutations.
    write_token = token or os.environ.get('GIT_TOKEN')
    if token or (method != 'GET' and write_token):
        environment = {**os.environ, 'GH_TOKEN': write_token}
    if payload is None:
        output = run(*args, env=environment)
    else:
        result = subprocess.run([*args, "--input", "-"], cwd=ROOT, input=json.dumps(payload),
                                text=True, capture_output=True, shell=False, env=environment)
        if result.returncode:
            raise ReleaseError(f"GitHub {method} {endpoint}: {result.stderr.strip()}")
        output = result.stdout
    return json.loads(output) if output.strip() else None


def pages(endpoint: str, *, token: str | None = None) -> list:
    environment = {**os.environ, 'GH_TOKEN': token} if token else None
    output = run("gh", "api", endpoint, "--paginate", "--slurp", env=environment)
    return [item for page in json.loads(output) for item in page]


@lru_cache(maxsize=32)
def run_jobs(run_id: int, attempt: int) -> dict[str, dict]:
    """Latest execution of each job, including successes skipped by failed-job reruns."""
    jobs = {}
    for number in range(attempt, 0, -1):
        page = 1
        while True:
            response = api(repository_path(f'actions/runs/{run_id}/attempts/{number}/jobs?per_page=100&page={page}'))
            for job in response['jobs']:
                jobs.setdefault(job['name'], {**job, 'evidence_attempt': number})
            if len(response['jobs']) < 100:
                break
            page += 1
    return jobs


def repository_path(path: str = "") -> str:
    return f"repos/{REPO}/{path}".rstrip("/")


def version(value: str) -> str:
    if not VERSION.fullmatch(value):
        raise ReleaseError("Version must be X.Y.Z or X.Y.Z-rc.N (N >= 1), without a v prefix.")
    return value


def file_at(path: str, ref: str) -> bytes:
    data = api(repository_path(f"contents/{quote(path, safe='/')}?ref={quote(ref, safe='')}"))
    if data.get("encoding") != "base64" or data.get("type") != "file":
        raise ReleaseError(f"Expected a regular base64-encoded repository file: {path}")
    return base64.b64decode(data["content"], validate=False)


def validate_intent(data: dict, expected_version: str | None = None) -> dict:
    if not isinstance(data, dict) or data.get("schema") != 1:
        raise ReleaseError("Unsupported release intent schema.")
    target = version(data.get("version", ""))
    channel = "rc" if "-rc." in target else "stable"
    if data.get("channel") != channel or data.get("mode") not in ("automatic", "explicit"):
        raise ReleaseError("Release intent channel/mode does not match its version.")
    predecessor = data.get("previous_stable")
    if predecessor is not None and ("-" in version(predecessor)):
        raise ReleaseError("The changelog predecessor must be a stable version.")
    if expected_version is not None and target != expected_version:
        raise ReleaseError("Release intent does not match the expected version.")
    return data


def release_branch(branch: str) -> bool:
    return branch.startswith("release-plz-") or branch.startswith("rc/")


def eligible_pr(pr: dict) -> bool:
    return (pr["base"]["ref"] == BASE
            and pr["base"]["repo"]["full_name"] == REPO
            and pr["head"].get("repo") is not None
            and pr["head"]["repo"]["full_name"] == REPO
            and release_branch(pr["head"]["ref"])
            and pr["user"]["login"] in CONFIG["release_authors"]
            and any(label["name"] == "release" for label in pr["labels"]))


def pr_intent(pr: dict) -> dict:
    if not eligible_pr(pr):
        raise ReleaseError("PR repository, author, branch, base, or release label is not eligible.")
    intent = validate_intent(json.loads(file_at(".github/release-intent.json", pr["head"]["sha"])))
    branch = pr["head"]["ref"]
    if intent["channel"] == "rc" and branch != f"rc/{intent['version']}":
        raise ReleaseError("RC intent must use its exact version branch.")
    if intent["channel"] == "stable" and not branch.startswith("release-plz-"):
        raise ReleaseError("Stable releases must use a release-plz branch.")
    return intent


def dispatch(workflow: str, ref: str, inputs: dict) -> None:
    api(repository_path(f"actions/workflows/{workflow}/dispatches"), method="POST",
        payload={"ref": ref, "inputs": inputs})


def git_repository(url: str) -> str | None:
    match = re.fullmatch(r"(?:git@github\.com:|https://github\.com/|ssh://git@github\.com/)([^/]+/[^/]+?)(?:\.git)?/?", url)
    return match.group(1) if match else None


def verify_local_repository() -> None:
    urls = run("git", "remote", "get-url", "--all", "origin").splitlines()
    push_urls = run("git", "remote", "get-url", "--push", "--all", "origin").splitlines()
    if len(urls) != 1 or len(push_urls) != 1 or any(git_repository(u) != REPO for u in urls + push_urls):
        raise ReleaseError(f"Both origin fetch and push must identify exactly {REPO}; run just doctor.")


def output(name: str, value: object) -> None:
    """Emit a compact trusted scalar to an Actions output file."""
    path = os.environ.get("GITHUB_OUTPUT")
    text = json.dumps(value, separators=(",", ":")) if not isinstance(value, str) else value
    if "\n" in text or "\r" in text:
        raise ReleaseError("Multiline Actions scalar output is not allowed.")
    if path:
        with open(path, "a") as stream:
            stream.write(f"{name}={text}\n")


def safe_text(value: object) -> str:
    return "".join(c for c in str(value) if c in "\n\t" or c.isprintable())
