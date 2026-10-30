#!/usr/bin/env python3
"""Stage CLI downloads separately from the SDK used only by CI verification."""
import argparse
import json
from pathlib import Path
import re
import shutil
import tempfile

from package import ROOT, TARGETS, make_sdk_archive, sha256


def stage_artifacts(root, target):
    suffix = ".zip" if TARGETS[target] == "windows" else ".tar.gz"
    names = {f"bili-{target}-release{suffix}", f"bili-native-source-{target}.tar.gz"}
    packages = root / "target/packages" / target
    sums = packages / "SHA256SUMS"
    recorded = {}
    for line in sums.read_text(encoding="ascii").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
        if not match or match[2] in recorded:
            raise SystemExit("invalid product SHA256SUMS")
        recorded[match[2]] = match[1]
    if set(recorded) != names:
        raise SystemExit("product checksums must cover only the CLI and matching native source")
    for name, expected in recorded.items():
        if not (packages / name).is_file() or sha256(packages / name) != expected:
            raise SystemExit(f"product checksum mismatch: {name}")

    sdk = root / "target/sdk" / target / "dylib"
    metadata = json.loads((sdk / "build-info.json").read_text(encoding="utf-8"))
    if metadata.get("target") != target or metadata.get("mode") != "dylib":
        raise SystemExit("SDK metadata does not match the product target and dylib mode")
    products = root / "target/ci-products" / target
    inputs = root / "target/ci-inputs" / target
    if products.exists() or inputs.exists():
        raise SystemExit("refusing to overwrite existing staged CI artifacts")
    with tempfile.TemporaryDirectory(prefix="ci-products-", dir=root / "target") as temporary:
        work = Path(temporary)
        product_stage = work / "product"
        input_stage = work / "inputs"
        product_stage.mkdir()
        input_stage.mkdir()
        make_sdk_archive(sdk, target, "dylib", input_stage)
        for name in sorted(names | {"SHA256SUMS"}):
            shutil.copy2(packages / name, product_stage / name)
        products.parent.mkdir(parents=True, exist_ok=True)
        inputs.parent.mkdir(parents=True, exist_ok=True)
        product_stage.rename(products)
        input_stage.rename(inputs)
    return products, inputs


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=TARGETS, required=True)
    args = parser.parse_args()
    products, inputs = stage_artifacts(ROOT, args.target)
    print(f"Product download: {products}")
    print(f"Internal SDK input: {inputs}")


if __name__ == "__main__":
    main()
