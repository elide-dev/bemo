#!/usr/bin/env python3
"""Bind reviewed source probes to one new final build, without learning lookup answers."""
import argparse
import importlib.util
import json
from pathlib import Path
import re
import shutil
import subprocess

spec = importlib.util.spec_from_file_location("symbolicate", Path(__file__).with_name("symbolicate.py"))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


def make_case(manifest, module, peer, probes, source_root, symbol_nm, foreign_id, dependency_root=None, snapshot_root=None, retention_path=None):
    data = json.loads(Path(manifest).read_text())
    module_hash = gate.sha(module)
    meta = gate.collect.native(module)
    matching = [m for m in data["modules"] if m["sha256"] == module_hash and m["architecture"] == meta["architecture"]]
    if not matching:
        raise ValueError("Binary is absent from this final distribution manifest")
    selected = [probe for probe in probes if any(re.search(probe["module"], m["path"]) for m in matching)]
    if not selected:
        raise ValueError("No reviewed probes cover this shipped module")
    revision = data["build"]["commit"]
    symbols = subprocess.check_output([symbol_nm, "--defined-only", "--demangle", str(peer)], text=True)
    rows = []
    for line in symbols.splitlines():
        fields = line.split(maxsplit=2)
        if len(fields) == 3 and re.fullmatch(r"[0-9a-fA-F]+", fields[0]) and fields[1].lower() in ("t", "w"):
            name = fields[2]
            if meta["format"] == "macho" and name.startswith("_"):
                name = name[1:]
            rows.append((int(fields[0], 16), name))
    case = {"repository": data["build"]["repository"], "sourceRevision": revision,
            "manifestSha256": gate.sha(manifest), "moduleSha256": module_hash, "debugSha256": gate.sha(peer),
            "modulePath": sorted(m["path"] for m in matching)[0], "architecture": meta["architecture"],
            "debugId": meta["identity"]["value"], "foreignDebugId": foreign_id, "frames": []}
    if gate.collect.native(peer)["identity"] != meta["identity"]:
        raise ValueError("Local retained peer differs from final module identity")
    if retention_path is not None:
        retention = json.loads(Path(retention_path).read_text())
        if retention["binary"]["sha256"] != module_hash or retention["debug"]["sha256"] != case["debugSha256"]:
            raise ValueError("Retained producer record differs from exact final module/peer")
        case["preservationRecordSha256"] = gate.sha(retention_path)
        gate.producer_quality(manifest, module_hash, retention_path)
    for probe in selected:
        addresses = {address for address, name in rows if re.fullmatch(probe["pcSymbol"], name)}
        if len(addresses) != 1:
            raise ValueError("Probe must select exactly one defined final code symbol")
        entry = addresses.pop()
        address = entry+probe.get("pcOffset", 0)
        nm_path = Path(shutil.which(symbol_nm))
        objdump = str(nm_path.with_name("llvm-objdump")) if nm_path.name.startswith("llvm-nm") else "objdump"
        assembly = subprocess.check_output([objdump, "--disassemble", "--no-show-raw-insn", "--start-address="+hex(entry),
                                            "--stop-address="+hex(address+16), str(module)], text=True)
        instructions = {int(value, 16) for value in re.findall(r"^\s*([0-9a-fA-F]+):\s", assembly, re.M)}
        if address not in instructions:
            raise ValueError("Reviewed PC offset is not a final executable instruction boundary")
        chain, sources = [], []
        for expected in probe["chain"]:
            source_revision, folder = revision, source_root
            if expected.get("dependency"):
                link = subprocess.check_output(["git", "-C", str(source_root), "ls-tree", revision, expected["dependency"]], text=True).split()
                if len(link) < 3 or link[0] != "160000" or dependency_root is None:
                    raise ValueError("Expected dependency's exact gitlink and source repository")
                source_revision, folder = link[2], dependency_root
            if expected.get("sourceSnapshot"):
                if snapshot_root is None or expected["sourceFile"] != "sqlite3.c":
                    raise ValueError("Regenerated owned C source snapshot is required")
                source = (snapshot_root/"sqlite3.c").read_bytes()
            else:
                source = subprocess.check_output(["git", "-C", str(folder), "show", source_revision+":"+expected["sourceFile"]])
            lines = [index for index, line in enumerate(source.decode().splitlines(), 1) if expected["sourceAnchor"] in line]
            if len(lines) != 1:
                raise ValueError("Reviewed source anchor must occur once")
            sources.append({"revision": source_revision, "file": expected["sourceFile"],
                            "sha256": gate.hashlib.sha256(source).hexdigest(),
                            "basis": "Reviewed semantic anchor: "+expected["sourceAnchor"]})
            if expected.get("dependency"):
                sources[-1]["dependency"] = expected["dependency"]
            if expected.get("sourceSnapshot"):
                sources[-1].update({"snapshot": True, "compilerMD5": gate.snapshot.source_md5(source),
                                   "provenanceSha256": gate.sha(snapshot_root/"provenance.json"), "compiledFunction": probe["compiledFunction"]})
            chain.append({"function": expected["function"], "file": expected["debugFile"],
                          "line": lines[0]+expected.get("lineOffset", 0)})
        case["frames"].append({"runtimePc": hex(address), "loadBias": "0x0", "sourceEvidence": sources[0],
                               "chainSources": sources[1:], "chain": chain,
                               "addressEvidence": {"method": "final retained symbol table; independent source expectations",
                                                   "pcSymbol": probe["pcSymbol"], "offset": probe.get("pcOffset", 0),
                                                   "entryPc": hex(entry), "finalInstructionBoundaryVerified": True}})
    gate.verify_source(case, source_root, dependency_root, snapshot_root)
    gate.verify_module(case, module, manifest)
    return case


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ("manifest", "module", "peer", "probes", "source-root", "out"):
        parser.add_argument("--"+key, type=Path, required=True)
    parser.add_argument("--dependency-source-root", type=Path)
    parser.add_argument("--source-snapshot-root", type=Path)
    parser.add_argument("--preservation-record", type=Path)
    parser.add_argument("--nm", default="llvm-nm")
    parser.add_argument("--foreign-debug-id", required=True)
    args = parser.parse_args()
    case = make_case(args.manifest, args.module, args.peer, json.loads(args.probes.read_text())["probes"],
                     args.source_root, args.nm, args.foreign_debug_id, args.dependency_source_root, args.source_snapshot_root, args.preservation_record)
    args.out.write_text(json.dumps(case, indent=2)+"\n")


if __name__ == "__main__":
    main()
