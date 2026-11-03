#!/usr/bin/env python3
"""Inventory every Linux platform of an immutable Docker image without running it.

Invoked by existing container packaging jobs only when they export an image.
Docker create/export reads the merged rootfs (including base-image libraries).
"""

import argparse
import importlib.util
import json
import re
import subprocess
import tempfile
from pathlib import Path


def run(*args):
    return subprocess.check_output(list(map(str, args)), text=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--image",
        required=True,
        help="image@sha256:digest or immutable local sha256:image-id",
    )
    parser.add_argument("--repository", required=True)
    parser.add_argument("--release", required=True)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--debug-root", action="append", default=[])
    args = parser.parse_args()
    local = bool(re.fullmatch(r"sha256:[a-f0-9]{64}", args.image))
    if not local and not re.fullmatch(r".+@sha256:[a-f0-9]{64}", args.image):
        parser.error("an immutable image digest is required")
    spec = importlib.util.spec_from_file_location(
        "coverage_collect", Path(__file__).with_name("collect.py")
    )
    collector = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(collector)
    raw = (
        {}
        if local
        else json.loads(
            run("docker", "buildx", "imagetools", "inspect", "--raw", args.image)
        )
    )
    descriptors = raw.get("manifests", [])
    platforms = [
        d["platform"] for d in descriptors if d.get("platform", {}).get("os") == "linux"
    ]
    if not descriptors:
        # A single manifest has no platform; inspect the pulled immutable image.
        if not local:
            run("docker", "pull", args.image)
        info = json.loads(run("docker", "image", "inspect", args.image))[0]
        platforms = [{"os": info["Os"], "architecture": info["Architecture"]}]
    if not platforms:
        raise ValueError("no supported Linux image platforms")
    args.out.mkdir(parents=True, exist_ok=True)
    for platform in platforms:
        coordinate = "/".join(
            platform[key]
            for key in ("os", "architecture", "variant")
            if platform.get(key)
        )
        if not local:
            run("docker", "pull", "--platform", coordinate, args.image)
        container = run(
            "docker",
            "create",
            "--platform",
            coordinate,
            "--entrypoint",
            "/__symbol_coverage_never_execute__",
            args.image,
        ).strip()
        try:
            with tempfile.TemporaryDirectory() as temporary:
                path = Path(temporary) / (
                    "rootfs-" + coordinate.replace("/", "-") + ".tar"
                )
                subprocess.run(
                    ["docker", "export", "-o", str(path), container], check=True
                )
                result = collector.collect(
                    Path.cwd(),
                    [str(path)],
                    args.debug_root,
                    args.repository,
                    {
                        "release": args.release,
                        "platform": platform["os"],
                        "architecture": coordinate,
                        "configuration": {
                            "image": args.image,
                            "export": "docker merged rootfs; no process executed",
                        },
                    },
                )
                result["gaps"].append(
                    {
                        "path": args.image,
                        "status": "unverified",
                        "reason": "Docker export excludes volume contents; runtime mounts and JIT code are outside build scope",
                    }
                )
                out = args.out / (coordinate.replace("/", "-") + ".json")
                out.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
        finally:
            subprocess.run(
                ["docker", "rm", container], check=True, stdout=subprocess.DEVNULL
            )


if __name__ == "__main__":
    main()
