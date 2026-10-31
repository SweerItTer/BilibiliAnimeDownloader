"""Shared metadata checks for product packages and release assets."""
import hashlib
import json
import shutil
from pathlib import Path
import re

TARGETS = {
    "x86_64-unknown-linux-gnu": ("linux", "tar.gz"),
    "x86_64-pc-windows-msvc": ("windows", "zip"),
}
PRODUCTS = ("cli", "gui", "tui")
PRODUCT = "cli"
PROTOCOL = 1
MAX_ASSET_NAME = 128
MAX_ARCHIVE_SIZE = 512 * 1024 * 1024


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def parse_doctor_identity(doctor_output, version_output, target, product=PRODUCT):
    if product not in PRODUCTS:
        raise ValueError(f"unsupported package product: {product}")
    try:
        version_line = version_output.strip()
        if not version_line.startswith("bili ") or not version_line[5:]:
            raise ValueError("invalid application version output")
        version = version_line[5:]
        lines = [line for line in doctor_output.splitlines() if line.strip()]
        if len(lines) != 1:
            raise ValueError("doctor must emit exactly one JSON record")
        envelope = json.loads(lines[0])
        data = envelope.get("data")
        identity = data.get("build_identity") if isinstance(data, dict) else None
        if (envelope.get("schema_version") != 1 or envelope.get("type") != "doctor"
                or not isinstance(identity, dict)
                or identity != {"product": product, "target": target,
                                "version": version, "update_protocol": PROTOCOL}
                or data.get("version") != version):
            raise ValueError("doctor build identity disagrees with the built executable")
        return version
    except (AttributeError, json.JSONDecodeError, TypeError) as error:
        raise ValueError("cannot validate application doctor build identity") from error


def descriptor_name(target, product=PRODUCT):
    if target not in TARGETS:
        raise ValueError(f"unsupported package target: {target}")
    if product not in PRODUCTS:
        raise ValueError(f"unsupported package product: {product}")
    return f"bili-update-{product}-{target}.json"


def descriptor_identity(name):
    if isinstance(name, str):
        for product in PRODUCTS:
            for target in TARGETS:
                if name == descriptor_name(target, product):
                    return product, target
    raise ValueError(f"unsupported update descriptor asset: {name}")


def safe_archive_name(name):
    return (isinstance(name, str) and 0 < len(name) <= MAX_ASSET_NAME
            and not name.startswith(".")
            and ".." not in name
            and re.fullmatch(r"[0-9A-Za-z._+-]+", name) is not None)


def descriptor_document(target, version, archive, product=PRODUCT):
    if target not in TARGETS:
        raise ValueError(f"unsupported package target: {target}")
    if product not in PRODUCTS:
        raise ValueError(f"unsupported package product: {product}")
    _, archive_format = TARGETS[target]
    archive = Path(archive)
    return {
        "schema_version": PROTOCOL,
        "product": product,
        "target": target,
        "version": version,
        "archive": {
            "name": archive.name,
            "format": archive_format,
            "sha256": sha256(archive),
            "size": archive.stat().st_size,
        },
    }


def validate_descriptor(document, target, version, archive_path=None, product=PRODUCT):
    if target not in TARGETS:
        raise ValueError(f"unsupported package target: {target}")
    if product not in PRODUCTS:
        raise ValueError(f"unsupported package product: {product}")
    _, archive_format = TARGETS[target]
    if (not isinstance(document, dict)
            or not {"schema_version", "product", "target", "version", "archive"} <= set(document)
            or type(document["schema_version"]) is not int or document["schema_version"] != PROTOCOL
            or document["product"] != product or document["target"] != target
            or not isinstance(document["version"], str) or not document["version"]
            or (version is not None and document["version"] != version)):
        raise ValueError("update descriptor identity/schema mismatch")
    archive = document["archive"]
    if (not isinstance(archive, dict)
            or not {"name", "format", "sha256", "size"} <= set(archive)
            or archive["format"] != archive_format
            or not safe_archive_name(archive["name"])
            or not isinstance(archive["sha256"], str)
            or not re.fullmatch(r"[0-9a-f]{64}", archive["sha256"])
            or type(archive["size"]) is not int
            or archive["size"] <= 0 or archive["size"] > MAX_ARCHIVE_SIZE):
        raise ValueError("update descriptor archive reference is invalid")
    if archive_path is not None:
        path = Path(archive_path)
        if (archive["name"] != path.name or archive["size"] != path.stat().st_size
                or archive["sha256"] != sha256(path)):
            raise ValueError("update descriptor does not match its package archive")
    return document


def load_descriptor(path, target, version, archive_path=None, product=PRODUCT):
    path = Path(path)
    if path.name != descriptor_name(target, product):
        raise ValueError("update descriptor filename does not match its target")
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError("update descriptor is missing or malformed") from error
    return validate_descriptor(document, target, version, archive_path, product)


def write_inventory(root):
    root = Path(root)
    inventory = root / "SHA256SUMS"
    rows = []
    for path in sorted(root.rglob("*")):
        if path == inventory or not path.is_file():
            continue
        relative = path.relative_to(root).as_posix()
        if "\\" in relative:
            raise ValueError(f"unsafe inventory path: {relative}")
        rows.append(f"{sha256(path)}  {relative}")
    inventory.write_text("\n".join(rows) + "\n", encoding="ascii")
    return inventory


def assemble_install_root(root, payload, bootstrap, target, product=PRODUCT):
    if target not in TARGETS:
        raise ValueError(f"unsupported package target: {target}")
    if product not in PRODUCTS:
        raise ValueError(f"unsupported package product: {product}")
    root, payload, bootstrap = map(Path, (root, payload, bootstrap))
    if not payload.is_dir() or payload.is_symlink() or not bootstrap.is_file() or bootstrap.is_symlink():
        raise ValueError("payload or bootstrap input is missing or unsafe")
    build_info = json.loads((payload / "BUILD_INFO.json").read_text(encoding="utf-8"))
    app_sha = build_info.get("app_sha256")
    if (build_info.get("product") != product
            or any(build_info.get(key) != PROTOCOL
                   for key in ("update_protocol", "layout_version", "bootstrap_protocol"))
            or not isinstance(app_sha, str) or not re.fullmatch(r"[0-9a-f]{64}", app_sha)):
        raise ValueError("payload build metadata is not update-compatible")
    version_id = f"v-{app_sha[:32]}"
    root.mkdir()
    payload_destination = root / "versions" / version_id
    payload_destination.parent.mkdir()
    payload.rename(payload_destination)
    if TARGETS[target][0] == "linux":
        bootstrap_destination = root / "bin/bili"
        bootstrap_destination.parent.mkdir()
    else:
        bootstrap_destination = root / "bili.exe"
    shutil.copy2(bootstrap, bootstrap_destination)
    marker = {
        "schema_version": PROTOCOL,
        "layout_version": PROTOCOL,
        "bootstrap_protocol": PROTOCOL,
        "product": product,
        "target": target,
    }
    (root / ".bili-install.json").write_text(
        json.dumps(marker, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    (root / "CURRENT").write_text(version_id + "\n", encoding="ascii")
    write_inventory(root)
    return payload_destination


def current_id(raw):
    if raw.endswith(b"\r\n"):
        raw = raw[:-2]
    elif raw.endswith(b"\n"):
        raw = raw[:-1]
    if b"\n" in raw or b"\r" in raw:
        raise ValueError("CURRENT must contain one version id")
    try:
        value = raw.decode("ascii")
    except UnicodeDecodeError as error:
        raise ValueError("CURRENT is not ASCII") from error
    if not re.fullmatch(r"v-[0-9a-f]{32}", value):
        raise ValueError("CURRENT is not a valid version id")
    return value


def validate_marker(marker, target, product=PRODUCT):
    if (not isinstance(marker, dict)
            or set(marker) != {"schema_version", "layout_version", "bootstrap_protocol", "product", "target"}
            or any(type(marker[key]) is not int or marker[key] != 1
                   for key in ("schema_version", "layout_version", "bootstrap_protocol"))
            or product not in PRODUCTS or marker["product"] != product or marker["target"] != target):
        raise ValueError("installation marker identity/schema mismatch")
    return marker


def installation_payload(root, target, product=PRODUCT):
    root = Path(root)
    marker_path = root / ".bili-install.json"
    current_path = root / "CURRENT"
    if root.is_symlink() or marker_path.is_symlink() or current_path.is_symlink():
        raise ValueError("installation marker or pointer is unsafe")
    validate_marker(json.loads(marker_path.read_text(encoding="utf-8")), target, product)
    current = current_id(current_path.read_bytes())
    bootstrap = root / ("bin/bili" if TARGETS[target][0] == "linux" else "bili.exe")
    versions = root / "versions"
    if (marker_path.is_symlink() or current_path.is_symlink() or bootstrap.is_symlink()
            or not bootstrap.is_file() or versions.is_symlink() or not versions.is_dir()
            or {path.name for path in versions.iterdir()} != {current}):
        raise ValueError("installation root does not match the versioned package layout")
    expected_root = {".bili-install.json", "CURRENT", "SHA256SUMS", "versions"}
    if TARGETS[target][0] == "linux":
        bin_dir = root / "bin"
        if bin_dir.is_symlink() or not bin_dir.is_dir() or {p.name for p in bin_dir.iterdir()} != {"bili"}:
            raise ValueError("Linux bootstrap layout is invalid")
        expected_root.add("bin")
    else:
        expected_root.add("bili.exe")
    if {path.name for path in root.iterdir()} != expected_root:
        raise ValueError("installation root contains unexpected files")
    payload = versions / current
    if payload.is_symlink() or not payload.is_dir():
        raise ValueError("current payload directory is missing or unsafe")
    return payload
