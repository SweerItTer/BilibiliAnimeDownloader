#!/usr/bin/env python3
"""Rebuild the complete static update catalog from public GitHub release metadata."""
import argparse
import json
import os
from pathlib import Path
import re
import sys
import tempfile
import urllib.error
import urllib.request

from package_metadata import (
    PRODUCTS,
    TARGETS,
    descriptor_identity,
    descriptor_name,
    validate_descriptor,
)

INDEX_SCHEMA = 1
MAX_RELEASES = 1000
MAX_INDEX_BYTES = 1024 * 1024
MAX_DESCRIPTOR_BYTES = 64 * 1024
MAX_PAGE_BYTES = 8 * 1024 * 1024
PER_PAGE = 100
MAX_TAG_LENGTH = 64

SEMVER = re.compile(
    r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
    r"(?:-(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
)


def tag_version(tag):
    """Return a canonical SemVer version for a tag, or None.

    This only validates spelling. It deliberately does not order versions.
    """
    if (not isinstance(tag, str) or not tag.startswith("v")
            or len(tag) > MAX_TAG_LENGTH):
        return None
    version = tag[1:]
    return version if SEMVER.fullmatch(version) else None


def is_prerelease(version):
    return "-" in version.partition("+")[0]


def encode_catalog(catalog, max_bytes=MAX_INDEX_BYTES):
    payload = (json.dumps(catalog, sort_keys=True, separators=(",", ":")) + "\n").encode()
    if len(payload) > max_bytes:
        raise ValueError("static update catalog exceeds its size bound")
    return payload


def build_catalog(releases, fetch_descriptor, max_releases=MAX_RELEASES):
    catalog = []
    seen = set()
    for release in releases:
        if not isinstance(release, dict):
            raise ValueError("GitHub release metadata is malformed")
        draft = release.get("draft")
        if not isinstance(draft, bool):
            raise ValueError("release is missing its draft flag")
        if draft:
            continue
        prerelease = release.get("prerelease")
        if not isinstance(prerelease, bool):
            raise ValueError("release is missing its prerelease flag")
        tag = release.get("tag_name")
        version = tag_version(tag)
        if version is None or is_prerelease(version) != prerelease:
            continue
        if tag in seen:
            raise ValueError(f"duplicate release tag: {tag}")
        seen.add(tag)
        if len(catalog) >= max_releases:
            raise ValueError("static update catalog exceeds its release bound")
        catalog.append({"tag": tag, "descriptors": release_descriptors(
            release, version, fetch_descriptor)})
    return {"schema_version": INDEX_SCHEMA, "releases": catalog}


def release_descriptors(release, version, fetch_descriptor):
    assets = release.get("assets")
    if not isinstance(assets, list) or not all(isinstance(asset, dict) for asset in assets):
        raise ValueError(f"release {release['tag_name']} has malformed asset metadata")
    by_name = {}
    for asset in assets:
        name = asset.get("name")
        if not isinstance(name, str) or not name:
            raise ValueError(f"release {release['tag_name']} has an unnamed asset")
        by_name.setdefault(name, []).append(asset)
    descriptors = []
    for product in PRODUCTS:
        for target in TARGETS:
            name = descriptor_name(target, product)
            matches = by_name.get(name, [])
            if len(matches) > 1:
                raise ValueError(f"release {release['tag_name']} repeats {name}")
            if not matches:
                continue
            asset = matches[0]
            url = asset.get("browser_download_url")
            if not isinstance(url, str) or not url.startswith("https://"):
                raise ValueError(f"release {release['tag_name']} cannot download {name}")
            body = fetch_descriptor(url, MAX_DESCRIPTOR_BYTES)
            try:
                document = json.loads(body)
            except (TypeError, UnicodeDecodeError, json.JSONDecodeError) as error:
                raise ValueError(f"release {release['tag_name']} has a malformed {name}") from error
            validate_descriptor(document, target, version, product=product)
            bind_archive(release, document["archive"], by_name)
            descriptors.append(document)
    for name in by_name:
        if name.startswith("bili-update-") and name.endswith(".json"):
            descriptor_identity(name)
    return descriptors


def bind_archive(release, archive, by_name):
    matches = by_name.get(archive["name"], [])
    if not matches:
        raise ValueError(f"release {release['tag_name']} omits archive {archive['name']}")
    if len(matches) > 1:
        raise ValueError(f"release {release['tag_name']} repeats archive {archive['name']}")
    asset = matches[0]
    if type(asset.get("size")) is not int or asset["size"] != archive["size"]:
        raise ValueError(f"release {release['tag_name']} archive size disagrees with its descriptor")
    digest = asset.get("digest")
    if digest is not None and digest != f"sha256:{archive['sha256']}":
        raise ValueError(f"release {release['tag_name']} archive digest disagrees with its descriptor")


def write_catalog(output, catalog, max_bytes=MAX_INDEX_BYTES):
    payload = encode_catalog(catalog, max_bytes)
    destination = Path(output) / "updates" / "v1" / "index.json"
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=destination.parent, delete=False) as temporary:
        temporary.write(payload)
        temporary.flush()
        name = temporary.name
    os.replace(name, destination)
    return destination


class GitHub:
    def __init__(self, repository, token, api_url="https://api.github.com"):
        if not re.fullmatch(r"[0-9A-Za-z_.-]+/[0-9A-Za-z_.-]+", repository):
            raise ValueError("repository must be an owner/name pair")
        self.repository = repository
        self.api_url = api_url.rstrip("/")
        self.token = token

    def request(self, url, accept, limit, authenticated=True):
        headers = {
            "Accept": accept,
            "User-Agent": "bili-release-publisher",
        }
        if authenticated:
            headers["Authorization"] = f"Bearer {self.token}"
            headers["X-GitHub-Api-Version"] = "2022-11-28"
        request = urllib.request.Request(url, headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                body = response.read(limit + 1)
        except urllib.error.HTTPError as error:
            raise ValueError(f"GitHub request failed with HTTP status {error.code}") from error
        except (urllib.error.URLError, TimeoutError) as error:
            raise ValueError("GitHub request failed") from error
        if len(body) > limit:
            raise ValueError("GitHub response exceeds its size bound")
        return body

    def releases(self):
        page = 1
        seen = set()
        while True:
            url = (f"{self.api_url}/repos/{self.repository}/releases"
                   f"?per_page={PER_PAGE}&page={page}")
            batch = json.loads(self.request(url, "application/vnd.github+json", MAX_PAGE_BYTES))
            if not isinstance(batch, list) or len(batch) > PER_PAGE:
                raise ValueError("GitHub release page is malformed")
            for release in batch:
                if not isinstance(release, dict) or type(release.get("id")) is not int:
                    raise ValueError("GitHub release metadata is malformed")
                if release["id"] in seen:
                    raise ValueError("GitHub release pagination repeated an entry")
                seen.add(release["id"])
            yield from batch
            if len(batch) < PER_PAGE:
                return
            page += 1

    def descriptor(self, url, limit):
        # Published release assets are public; never send the CI token to a
        # browser download URL or to the object host it redirects to.
        return self.request(url, "application/octet-stream", limit, authenticated=False)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--output", type=Path, required=True,
                        help="GitHub Pages artifact root; writes updates/v1/index.json")
    parser.add_argument("--api-url", default=os.environ.get("GITHUB_API_URL", "https://api.github.com"))
    args = parser.parse_args()
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if not token:
        raise SystemExit("GH_TOKEN or GITHUB_TOKEN is required")
    client = GitHub(args.repository, token, args.api_url)
    catalog = build_catalog(client.releases(), client.descriptor)
    destination = write_catalog(args.output, catalog)
    print(f"{destination}: {len(catalog['releases'])} releases, "
          f"{destination.stat().st_size} bytes")


if __name__ == "__main__":
    sys.exit(main())
