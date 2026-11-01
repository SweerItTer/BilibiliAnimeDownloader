"""Preserve the exact SDK notices for the GUI's actually linked WebView2 loader."""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def collect(package, target, output_root, root=ROOT):
    lock = json.loads((root / "scripts/webview2-sdk-lock.json").read_text(encoding="utf-8"))
    if (lock.get("format") != 1 or package["name"] != lock["package"] or
            package["version"] != lock["crate_version"] or target != lock["target"]):
        raise ValueError("WebView2 SDK inputs do not match the pinned GUI target/dependency")
    crate_root = Path(package["manifest_path"]).resolve(strict=True).parent
    loader = crate_root / lock["loader"]["crate_path"]
    if (loader.is_symlink() or not loader.resolve(strict=True).is_relative_to(crate_root) or
            hashlib.sha256(loader.read_bytes()).hexdigest() != lock["loader"]["sha256"]):
        raise ValueError("WebView2 loader differs from the pinned Microsoft SDK input")
    records = []
    for notice in lock["notices"]:
        name = notice["filename"]
        if Path(name).name != name:
            raise ValueError("invalid WebView2 notice name")
        source = root / "scripts/notices/webview2-sdk" / name
        data = source.read_bytes()
        data.decode("utf-8")
        if hashlib.sha256(data).hexdigest() != notice["sha256"]:
            raise ValueError("WebView2 SDK notice differs from its original pinned bytes")
        destination = output_root / "licenses/native/webview2-sdk" / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(data)
        records.append({"path": destination.relative_to(output_root).as_posix(),
                        "sha256": notice["sha256"],
                        "provenance": f"Microsoft.Web.WebView2 {lock['version']} NuGet member {name}, unchanged"})
    return {"name": "Microsoft.Web.WebView2-loader", "version": lock["version"],
            "source": lock["url"], "checksum": lock["sha256"], "spdx": None,
            "license_basis": "original Microsoft SDK LICENSE.txt and NOTICE.txt; no replacement license is asserted",
            "loader_sha256": lock["loader"]["sha256"], "texts": records}
