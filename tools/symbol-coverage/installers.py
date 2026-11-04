#!/usr/bin/env python3
"""Inventory final installer bytes, using platform decoders without installing them.

Missing decoders and unsupported envelopes remain hashed, explicit coverage gaps.
The collector's normal archive walker handles Debian packages directly. Platform
tools are used for RPM, Apple PKG/DMG and MSI, including embedded CAB payloads.
"""

import argparse
import importlib.util
import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "coverage_collect", Path(__file__).with_name("collect.py")
)
collector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(collector)


def run(*args):
    subprocess.run(list(map(str, args)), check=True, stdout=subprocess.DEVNULL)


def relabel(value, prefix, envelope):
    if isinstance(value, dict):
        for key, item in value.items():
            if key == "path" and isinstance(item, str) and item.startswith(prefix):
                value[key] = envelope + "!/" + item[len(prefix) :].lstrip("/!")
            else:
                relabel(item, prefix, envelope)
    elif isinstance(value, list):
        for item in value:
            relabel(item, prefix, envelope)


def inspect(root, installer, repository, metadata, debug_roots=()):
    root = Path(root).resolve()
    installer = Path(installer).resolve()
    label = installer.relative_to(root).as_posix()
    result = collector.collect(root, [label], debug_roots, repository, metadata)
    result["evidence"].append(
        {**collector.artifact(installer, label), "status": "collected"}
    )
    suffix = installer.suffix.lower()
    if suffix == ".deb":
        return result
    with tempfile.TemporaryDirectory(prefix="symbol-installer-") as temporary:
        temp = Path(temporary)
        payload = temp / "payload"
        mounted = False
        cabinets = []
        try:
            if suffix == ".rpm":
                # libarchive converts RPM's compressed cpio to a tar stream. No
                # package scripts run, and no archive paths are extracted to disk.
                payload = temp / "payload.tar"
                run("bsdtar", "-cf", payload, "@" + str(installer))
            elif suffix == ".pkg":
                run("pkgutil", "--expand-full", installer, payload)
            elif suffix == ".dmg":
                payload.mkdir()
                run(
                    "hdiutil",
                    "attach",
                    "-readonly",
                    "-nobrowse",
                    "-noautoopen",
                    "-mountpoint",
                    payload,
                    installer,
                )
                mounted = True
            elif suffix == ".msi":
                seven = shutil.which("7z") or shutil.which("7z.exe")
                installed = Path(os.environ.get("ProgramFiles", "")) / "7-Zip/7z.exe"
                if not seven and installed.is_file():
                    seven = str(installed)
                if not seven:
                    raise FileNotFoundError(
                        "7z is required to inspect MSI streams/CAB payloads"
                    )
                payload.mkdir()
                run(seven, "x", "-y", "-o" + str(payload), installer)
                # MSI Binary streams may themselves be native custom actions;
                # retain them as well as every file inside embedded cabinets.
                for cabinet in list(payload.rglob("*")):
                    if not cabinet.is_file():
                        continue
                    with cabinet.open("rb") as stream:
                        is_cabinet = stream.read(4) == b"MSCF"
                    if is_cabinet:
                        destination = cabinet.with_name(cabinet.name + "-files")
                        run(seven, "x", "-y", "-o" + str(destination), cabinet)
                        cabinets.append((cabinet, destination))
            else:
                raise ValueError("installer payload decoder not implemented")
            decoded = collector.collect(
                root, [str(payload)], debug_roots, repository, metadata
            )
            for cabinet, destination in cabinets:
                cabinet_label = (
                    payload.name + "/" + cabinet.relative_to(payload).as_posix()
                )
                destination_label = (
                    payload.name + "/" + destination.relative_to(payload).as_posix()
                )
                relabel(decoded, destination_label, cabinet_label)
                decoded["containers"].append(
                    {**collector.artifact(cabinet, cabinet_label), "format": "cab"}
                )
                decoded["gaps"] = [
                    g
                    for g in decoded["gaps"]
                    if not (
                        g["path"] == cabinet_label
                        and g["reason"]
                        == "unrecognized or unsupported native/debug/archive format"
                    )
                ]
            relabel(decoded, payload.name, label)
            for key in (
                "modules",
                "inputs",
                "containers",
                "debugArtifacts",
                "aliases",
                "gaps",
            ):
                result[key].extend(decoded[key])
            # The original unsupported-envelope gap is resolved only after the
            # decoder and the inventory both complete. Other gaps remain intact.
            result["gaps"] = [
                g
                for g in result["gaps"]
                if not (
                    g["path"] == label
                    and g["reason"]
                    == "unrecognized or unsupported native/debug/archive format"
                )
            ]
            if result["modules"]:
                result["gaps"] = [
                    g
                    for g in result["gaps"]
                    if g["reason"] != "no final native modules discovered"
                ]
        except (OSError, ValueError, subprocess.CalledProcessError) as error:
            result["gaps"].append(
                {
                    "path": label,
                    "status": "unverified",
                    "reason": f"installer payload inspection unavailable: {error}",
                }
            )
        finally:
            if mounted:
                run("hdiutil", "detach", payload)
    collector.validate(result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--repository", required=True)
    parser.add_argument("--release", required=True)
    parser.add_argument("--platform", required=True)
    parser.add_argument("--architecture", required=True)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--debug-root", action="append", default=[])
    args = parser.parse_args()
    metadata = {
        "release": args.release,
        "platform": args.platform,
        "architecture": args.architecture,
        "configuration": {"boundary": "final installer; never installed"},
    }
    args.out.mkdir(parents=True, exist_ok=True)
    installers = sorted(
        p
        for p in args.directory.rglob("*")
        if p.is_file() and p.suffix.lower() in (".deb", ".rpm", ".pkg", ".dmg", ".msi")
    )
    if not installers:
        parser.error("no final installers found at the required installer boundary")
    for installer in installers:
        result = inspect(
            args.root, installer, args.repository, metadata, args.debug_root
        )
        output = args.out / (installer.name + ".symbol-coverage.json")
        output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
        print(f"Installer coverage: {len(result['modules'])} modules -> {output}")


if __name__ == "__main__":
    main()
