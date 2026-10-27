#!/usr/bin/env python3
"""Collect pinned Microsoft Windows runtime redistribution terms as release notices."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import tempfile
import urllib.request
import xml.etree.ElementTree as ET
import zipfile

ROOT = Path(__file__).resolve().parents[1]
LOCK = ROOT / "scripts/windows-redist-lock.json"
WORD = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"


def _sha256(data):
    return hashlib.sha256(data).hexdigest()


def _read_lock():
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    if lock.get("format") != 1 or not isinstance(lock.get("sources"), list) or not lock["sources"]:
        raise ValueError("invalid Windows redistribution source lock")
    ids, filenames = set(), set()
    for source in lock["sources"]:
        if (not re.fullmatch(r"[a-z0-9-]+", source.get("id", "")) or
                source["id"] in ids or Path(source.get("filename", "")).name != source.get("filename") or
                source["filename"] in filenames or not source["url"].startswith("https://") or
                not re.fullmatch(r"[0-9a-f]{64}", source.get("sha256", "")) or
                source.get("kind") not in {"docx", "original-bytes-only"} or not source.get("scope")):
            raise ValueError("invalid or duplicate entry in Windows redistribution source lock")
        ids.add(source["id"])
        filenames.add(source["filename"])
    return lock["sources"]


def _fetch(source, cache):
    cached = cache / source["filename"]
    if cached.exists():
        data = cached.read_bytes()
        if _sha256(data) != source["sha256"]:
            raise ValueError(f"cached Microsoft source has unexpected SHA-256: {source['filename']}")
        return cached
    request = urllib.request.Request(source["url"], headers={"User-Agent": "BilibiliAnimeDownloader-license-collector/1"})
    with urllib.request.urlopen(request, timeout=60) as response:
        data = response.read()
    if _sha256(data) != source["sha256"]:
        raise ValueError(f"downloaded Microsoft source has unexpected SHA-256: {source['filename']}")
    cache.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=f".{source['filename']}.", dir=cache)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
        try:
            os.link(temporary, cached)
        except FileExistsError:
            if _sha256(cached.read_bytes()) != source["sha256"]:
                raise ValueError(f"concurrent cache source has unexpected SHA-256: {source['filename']}")
    finally:
        Path(temporary).unlink(missing_ok=True)
    return cached


def _run_text(element):
    chunks = []
    for node in element.iter():
        if node.tag == WORD + "t":
            chunks.append(node.text or "")
        elif node.tag == WORD + "tab":
            chunks.append("\t")
        elif node.tag in (WORD + "br", WORD + "cr"):
            chunks.append("\n")
    return "".join(chunks)


def _paragraphs(container):
    if container is None:
        return
    for paragraph in container.iter(WORD + "p"):
        text = _run_text(paragraph).strip()
        if text:
            yield text


def _docx_text(path):
    sections = []
    with zipfile.ZipFile(path) as archive:
        names = set(archive.namelist())
        document = ET.fromstring(archive.read("word/document.xml"))
        body = document.find(WORD + "body")
        if body is None:
            raise ValueError(f"DOCX has no WordprocessingML body: {path.name}")
        blocks = []
        for child in body:
            if child.tag == WORD + "p":
                blocks.extend(_paragraphs(child))
            elif child.tag == WORD + "tbl":
                for row in child.findall(WORD + "tr"):
                    cells = []
                    for cell in row.findall(WORD + "tc"):
                        cells.append(" ".join(_paragraphs(cell)))
                    if any(cells):
                        blocks.append("\t|\t".join(cells))
        if not blocks:
            raise ValueError(f"DOCX body has no extractable text: {path.name}")
        sections.append(("Document", blocks))
        for part in ("footnotes", "endnotes"):
            name = f"word/{part}.xml"
            if name in names:
                items = []
                root = ET.fromstring(archive.read(name))
                for note in root:
                    note_type = note.attrib.get(WORD + "type")
                    if note_type in ("separator", "continuationSeparator"):
                        continue
                    text = list(_paragraphs(note))
                    if text:
                        items.extend(text)
                if items:
                    sections.append((part.title(), items))
        for name in sorted(name for name in names if re.fullmatch(r"word/(?:header|footer)\d+\.xml", name)):
            text = list(_paragraphs(ET.fromstring(archive.read(name))))
            if text:
                sections.append((Path(name).stem.title(), text))
    output = []
    for title, paragraphs in sections:
        output.extend((f"[{title}]", "", *paragraphs, ""))
    return "\n".join(output).rstrip() + "\n"


def collect_windows_redist_notices(destination: Path, cache: Path | None = None) -> list[Path]:
    """Atomically create notices from exact pinned Microsoft source documents."""
    destination = Path(destination).absolute()
    if destination.exists() or destination.is_symlink():
        raise FileExistsError(f"notice destination must not already exist: {destination}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    lock_path = destination.with_name(destination.name + ".lock")
    lock_fd = os.open(lock_path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    os.close(lock_fd)
    stage = None
    try:
        if destination.exists() or destination.is_symlink():
            raise FileExistsError(f"notice destination appeared during collection: {destination}")
        cache = Path(cache).absolute() if cache is not None else ROOT / "target/license-cache"
        stage = Path(tempfile.mkdtemp(prefix=f".{destination.name}.", dir=destination.parent))
        sources = _read_lock()
        notices = stage / "notices"
        upstream = stage / "source"
        notices.mkdir()
        upstream.mkdir()
        provenance = {"format": 1, "sources": []}
        text_paths = []
        for source in sources:
            cached = _fetch(source, cache)
            raw = cached.read_bytes()
            if _sha256(raw) != source["sha256"]:
                raise ValueError(f"Microsoft source changed while collecting: {source['filename']}")
            raw_name = f"{source['id']}{Path(source['filename']).suffix}"
            (upstream / raw_name).write_bytes(raw)
            record = {
                "id": source["id"], "kind": source["kind"], "scope": source["scope"], "url": source["url"],
                "source_sha256": source["sha256"], "source_path": f"source/{raw_name}",
            }
            if source["kind"] == "docx":
                text = _docx_text(cached).encode("utf-8")
                text_name = f"{source['id']}.txt"
                (notices / text_name).write_bytes(text)
                record.update({
                    "text_path": f"notices/{text_name}", "text_sha256": _sha256(text),
                    "extraction": "WordprocessingML document paragraphs and table cells in document order; footnotes, endnotes, headers and footers included where present; paragraphs retained as UTF-8 plain text. Word automatic list numbering is not reconstructed; the exact source DOCX is retained alongside this readability derivative.",
                })
                text_paths.append(destination / "notices" / text_name)
            provenance["sources"].append(record)
        (stage / "provenance.json").write_text(json.dumps(provenance, ensure_ascii=False, sort_keys=True, indent=2) + "\n", encoding="utf-8")
        if destination.exists() or destination.is_symlink():
            raise FileExistsError(f"notice destination appeared during collection: {destination}")
        os.rename(stage, destination)
        return text_paths
    finally:
        if stage is not None and stage.exists():
            shutil.rmtree(stage, ignore_errors=True)
        lock_path.unlink(missing_ok=True)
