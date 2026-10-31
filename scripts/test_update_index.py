import json
from pathlib import Path
import tempfile
import unittest

from package_metadata import TARGETS, descriptor_name
from update_index import (
    MAX_INDEX_BYTES,
    GitHub,
    build_catalog,
    encode_catalog,
    is_prerelease,
    tag_version,
    write_catalog,
)


LINUX = "x86_64-unknown-linux-gnu"
WINDOWS = "x86_64-pc-windows-msvc"


def release(tag, *, draft=False, prerelease=None, assets=()):
    return {
        "id": id(tag + str(assets)),
        "tag_name": tag,
        "draft": draft,
        "prerelease": "-" in tag.partition("+")[0] if prerelease is None else prerelease,
        "assets": list(assets),
    }


def asset(name, size=10, digest=None, url=None):
    document = {
        "name": name,
        "size": size,
        "browser_download_url": url or f"https://example.test/{name}",
    }
    if digest is not None:
        document["digest"] = digest
    return document


def descriptor(target, version, product="cli", archive=None, padding=""):
    archive = archive or f"bili-{product}-{target}-release.{TARGETS[target][1]}"
    document = {
        "schema_version": 1,
        "product": product,
        "target": target,
        "version": version,
        "archive": {
            "name": archive,
            "format": TARGETS[target][1],
            "sha256": "a" * 64,
            "size": 10,
        },
    }
    if padding:
        document["padding"] = padding
    return document


def build(releases, descriptors=None, **limits):
    descriptors = descriptors or {}
    return build_catalog(
        releases,
        lambda url, limit: descriptors[url],
        **limits,
    )


def published(release_document, descriptor_documents, extra_assets=()):
    base = f"https://example.test/{release_document['tag_name']}"
    assets = list(extra_assets)
    for document in descriptor_documents:
        name = descriptor_name(document["target"], document["product"])
        digest = f"sha256:{document['archive']['sha256']}"
        assets.append(asset(name, digest=digest, url=f"{base}/{name}"))
        assets.append(asset(document["archive"]["name"], digest=digest))
    bodies = {
        f"{base}/{descriptor_name(document['target'], document['product'])}":
            json.dumps(document).encode()
        for document in descriptor_documents
    }
    release_document["assets"] = assets
    return release_document, assets, bodies


class CatalogTests(unittest.TestCase):
    def test_keeps_published_tags_and_skips_drafts_inconsistent_or_noncanonical(self):
        releases = [
            release("v2.0.0"),
            release("v2.1.0-alpha.1"),
            release("v3.0.0", draft=True),
            release("release-2"),
            release("v2.2.0-beta.1", prerelease=False),
            release("v2.3.0", prerelease=True),
        ]
        catalog = build(releases)
        self.assertEqual(catalog["schema_version"], 1)
        self.assertEqual(
            [entry["tag"] for entry in catalog["releases"]],
            ["v2.0.0", "v2.1.0-alpha.1"],
        )
        self.assertTrue(all(entry["descriptors"] == [] for entry in catalog["releases"]))

    def test_publishes_every_supported_product_and_target(self):
        cli = descriptor(LINUX, "2.0.0")
        gui = descriptor(WINDOWS, "2.0.0", product="gui", archive="bili-gui-windows.zip")
        tag, assets, bodies = published(release("v2.0.0"), [cli, gui])
        catalog = build([tag], bodies)
        self.assertEqual(
            [(document["product"], document["target"])
             for document in catalog["releases"][0]["descriptors"]],
            [("cli", LINUX), ("gui", WINDOWS)],
        )

    def test_rejects_invalid_descriptor_or_archive_binding(self):
        base, assets, bodies = published(release("v2.0.0"), [descriptor(LINUX, "2.0.0")])
        url = "https://example.test/v2.0.0/" + descriptor_name(LINUX, "cli")
        cases = {
            "malformed descriptor": (
                base, {**bodies, url: b"{"}, assets),
            "identity mismatch": (
                base, {**bodies, url: json.dumps(descriptor(LINUX, "2.0.0", product="gui")).encode()},
                assets),
            "version mismatch": (
                base, {**bodies, url: json.dumps(descriptor(LINUX, "2.0.1")).encode()},
                assets),
            "missing archive": (
                base, bodies,
                [item for item in assets if item["name"] != descriptor(LINUX, "2.0.0")["archive"]["name"]]),
            "duplicate archive": (
                base, bodies, assets + [asset(descriptor(LINUX, "2.0.0")["archive"]["name"])]),
            "size mismatch": (
                base, bodies,
                [asset(item["name"], 11, item.get("digest"), item["browser_download_url"])
                 for item in assets]),
            "digest mismatch": (
                base, bodies,
                [asset(item["name"], item["size"], "sha256:" + "0" * 64,
                       item["browser_download_url"]) for item in assets]),
            "duplicate descriptor": (
                base, bodies, assets + [asset(descriptor_name(LINUX, "cli"))]),
            "unknown descriptor": (
                base, bodies, assets + [asset("bili-update-cli-riscv64.json")]),
        }
        for label, (release_document, responses, release_assets) in cases.items():
            with self.subTest(label=label):
                release_document = {**release_document, "assets": release_assets}
                with self.assertRaises(ValueError):
                    build([release_document], responses)

    def test_rejects_duplicate_tags_missing_flags_and_release_bound(self):
        with self.assertRaises(ValueError):
            build([release("v2.0.0"), release("v2.0.0", assets=[asset("bili.tar.gz")])])
        malformed = release("v2.0.0")
        del malformed["draft"]
        with self.assertRaises(ValueError):
            build([malformed])
        with self.assertRaises(ValueError):
            build([release(f"v0.0.{index}") for index in range(1001)])

    def test_rejects_catalog_above_the_decoded_size_bound(self):
        padding = "x" * 4096
        releases, bodies = [], {}
        for index in range(300):
            tag, _assets, descriptors = published(
                release(f"v1.0.{index}"), [descriptor(LINUX, f"1.0.{index}", padding=padding)])
            releases.append(tag)
            bodies.update(descriptors)
        catalog = build(releases, bodies)
        self.assertGreater(len(encode_catalog(catalog, max_bytes=MAX_INDEX_BYTES * 2)), MAX_INDEX_BYTES)
        with self.assertRaises(ValueError):
            encode_catalog(catalog)
        with self.assertRaises(ValueError):
            encode_catalog(catalog, max_bytes=16)

    def test_writes_the_published_site_path(self):
        with tempfile.TemporaryDirectory() as temporary:
            catalog = build([release("v2.0.0")])
            destination = write_catalog(temporary, catalog)
            self.assertEqual(destination, Path(temporary) / "updates/v1/index.json")
            self.assertEqual(json.loads(destination.read_text()), catalog)

    def test_tag_validation_is_spelling_only(self):
        for tag, version in (
            ("v2.0.0", "2.0.0"),
            ("v2.0.0-alpha.1+build.5", "2.0.0-alpha.1+build.5"),
            ("v0.0.0-0.alpha", "0.0.0-0.alpha"),
        ):
            self.assertEqual(tag_version(tag), version)
        for tag in ("2.0.0", "v02.0.0", "v2.0", "v2.0.0-01", "v2.0.0-",
                    "v1.0.0-" + "a" * 60):
            self.assertIsNone(tag_version(tag))
        self.assertTrue(is_prerelease("2.0.0-alpha+build"))
        self.assertFalse(is_prerelease("2.0.0+build"))


class PaginationTests(unittest.TestCase):
    class Client(GitHub):
        def __init__(self, pages):
            super().__init__("owner/repository", "secret")
            self.pages = list(pages)
            self.urls = []

        def request(self, url, accept, limit, authenticated=True):
            self.urls.append(url)
            return json.dumps(self.pages.pop(0)).encode()

    def test_reads_every_page_until_a_short_page(self):
        client = self.Client([[{"id": index} for index in range(100)], [{"id": 100}]])
        self.assertEqual(len(list(client.releases())), 101)
        self.assertEqual(len(client.urls), 2)
        self.assertIn("page=1", client.urls[0])
        self.assertIn("page=2", client.urls[1])

    def test_rejects_repeated_pagination(self):
        page = [{"id": index} for index in range(100)]
        client = self.Client([page, page])
        with self.assertRaises(ValueError):
            list(client.releases())


if __name__ == "__main__":
    unittest.main()
