#!/usr/bin/env python3
"""Archive completed CI handoff ZIPs in GHCR before removing their Actions copies."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import urllib.error
import urllib.parse
import urllib.request

FINAL_ARTIFACTS = {"linux-cli", "windows-cli", "windows-gui"}
WORKFLOWS = ("cli-packages.yml", "release.yml")
DIGEST = re.compile(r"sha256:[0-9a-f]{64}")


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, url):
        return None


class GitHub:
    def __init__(self, repository, token):
        self.repository = repository
        self.token = token
        self.opener = urllib.request.build_opener(NoRedirect())

    def request(self, path, method="GET"):
        request = urllib.request.Request(
            f"https://api.github.com/repos/{self.repository}" + (f"/{path}" if path else ""), method=method,
            headers={"Authorization": f"Bearer {self.token}",
                     "Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28"},
        )
        return self.opener.open(request, timeout=120)

    def json(self, path, method="GET"):
        try:
            with self.request(path, method) as response:
                return json.load(response) if method != "DELETE" else None
        except urllib.error.HTTPError as error:
            raise SystemExit(f"GitHub {method} {path} failed (HTTP {error.code})") from None

    def download(self, artifact_id, output):
        try:
            response = self.request(f"actions/artifacts/{artifact_id}/zip")
        except urllib.error.HTTPError as error:
            if error.code != 302:
                raise SystemExit(f"Artifact {artifact_id} download failed (HTTP {error.code})") from None
            location = error.headers.get("Location", "")
            error.close()
            parsed = urllib.parse.urlparse(location)
            if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password:
                raise SystemExit("Artifact download returned an unsafe storage redirect")
            # The signed storage URL must never receive the GitHub bearer token.
            response = urllib.request.urlopen(location, timeout=120)
        with response, output.open("wb") as destination:
            shutil.copyfileobj(response, destination)

    def artifacts(self, run_id):
        result = []
        page = 1
        while True:
            batch = self.json(f"actions/runs/{run_id}/artifacts?per_page=100&page={page}")["artifacts"]
            result.extend(batch)
            if len(batch) < 100:
                return result
            page += 1


class Registry:
    def __init__(self, repository, actor, token):
        self.repository = repository
        self.actor = actor
        self.token = token

    def command(self, command, *arguments, cwd=None):
        args = ["oras", command, *arguments]
        if self.token:
            args.extend(("--username", self.actor, "--password-stdin"))
        result = subprocess.run(args, input=self.token, text=True, capture_output=True, cwd=cwd)
        if result.returncode:
            raise SystemExit(f"oras {command} failed (exit {result.returncode}); Actions input retained")
        return result.stdout.strip()

    def archive(self, artifact, run, payload):
        tag = f"run-{run['id']}-attempt-{run['run_attempt']}-artifact-{artifact['id']}"
        reference = f"{self.repository}:{tag}"
        annotations = {
            "org.opencontainers.image.source": run["repository"]["html_url"],
            "org.opencontainers.image.revision": run["head_sha"],
            "org.opencontainers.image.created": run["created_at"],
            "io.github.actions.run": run["html_url"],
            "io.github.actions.artifact.name": artifact["name"],
            "io.github.actions.artifact.sha256": sha256(payload),
        }
        arguments = ["--format", "json", "--artifact-type", "application/vnd.github.actions.artifact-archive.v1"]
        for key, value in annotations.items():
            arguments.extend(("--annotation", f"{key}={value}"))
        pushed = json.loads(self.command("push", *arguments, reference,
                                         f"{payload.name}:application/zip", cwd=payload.parent))
        digest = pushed.get("digest", "")
        if not DIGEST.fullmatch(digest) or self.command("resolve", reference) != digest:
            raise SystemExit("Registry manifest digest did not verify; Actions input retained")
        pinned = f"{self.repository}@{digest}"
        with tempfile.TemporaryDirectory(prefix="oci-readback-", dir=payload.parent) as temporary:
            self.command("pull", pinned, "--output", temporary)
            restored = Path(temporary) / payload.name
            if not restored.is_file() or restored.is_symlink() or sha256(restored) != annotations["io.github.actions.artifact.sha256"]:
                raise SystemExit("Registry readback differs from the Actions ZIP; Actions input retained")
        return pinned


def archive_run(api, registry, run_id):
    repository = api.json("")
    run = api.json(f"actions/runs/{run_id}")
    if (run.get("id") != run_id or run.get("repository", {}).get("id") != repository["id"]
            or run.get("status") != "completed" or run.get("conclusion") != "success"):
        raise SystemExit("Archive requires a successful completed run in this repository")
    allowed = [api.json(f"actions/workflows/{name}") for name in WORKFLOWS]
    if not any(workflow["id"] == run.get("workflow_id")
               and workflow["path"] == f".github/workflows/{name}"
               for workflow, name in zip(allowed, WORKFLOWS)):
        raise SystemExit("Refusing to archive an unrelated workflow")
    if not isinstance(run.get("run_attempt"), int) or run["run_attempt"] < 1:
        raise SystemExit("Invalid workflow attempt")
    artifacts = api.artifacts(run_id)
    if run["workflow_id"] == allowed[0]["id"] and artifacts and not any(
            artifact["name"] in FINAL_ARTIFACTS for artifact in artifacts):
        raise SystemExit("No recognized product downloads; refusing legacy or incomplete-run cleanup")
    archived = []
    with tempfile.TemporaryDirectory(prefix="bili-ci-archive-") as temporary:
        for artifact in artifacts:
            if artifact["name"] in FINAL_ARTIFACTS:
                continue
            artifact_id = artifact["id"]
            if not isinstance(artifact_id, int) or artifact_id < 1 or artifact.get("expired"):
                raise SystemExit("Cannot safely archive an expired or invalid artifact")
            payload = Path(temporary) / f"artifact-{artifact_id}.zip"
            api.download(artifact_id, payload)
            expected = artifact.get("digest")
            if expected and expected != f"sha256:{sha256(payload)}":
                raise SystemExit(f"Artifact {artifact_id} does not match GitHub's recorded checksum")
            pinned = registry.archive(artifact, run, payload)
            api.json(f"actions/artifacts/{artifact_id}", method="DELETE")
            archived.append({"artifact_id": artifact_id, "reference": pinned, "sha256": sha256(payload)})
            payload.unlink()
            print(json.dumps(archived[-1]), flush=True)
    return archived


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--run-id", type=int, required=True)
    args = parser.parse_args()
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", args.repository) or args.run_id < 1:
        parser.error("a repository owner/name and positive run ID are required")
    token = os.environ.get("GITHUB_TOKEN")
    actor = os.environ.get("GITHUB_ACTOR")
    if not token or not actor:
        parser.error("GITHUB_TOKEN and GITHUB_ACTOR are required")
    api = GitHub(args.repository, token)
    registry = Registry(f"ghcr.io/{args.repository.lower()}/ci-artifacts", actor, token)
    archived = archive_run(api, registry, args.run_id)
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(summary).open("a", encoding="utf-8") as output:
            output.write("## Durable CI inputs\n\n")
            for item in archived:
                output.write(f"- Artifact {item['artifact_id']}: `{item['reference']}`\n")
            if not archived:
                output.write("No intermediate Actions artifacts need archival.\n")
    print(f"Archived and verified {len(archived)} intermediate artifacts; product downloads retained.")


if __name__ == "__main__":
    try:
        main()
    except (urllib.error.URLError, TimeoutError):
        raise SystemExit("Artifact archival network request failed; unverified Actions inputs retained") from None
