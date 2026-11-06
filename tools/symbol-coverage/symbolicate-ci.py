#!/usr/bin/env python3
"""Prepare sampled address gates only after exact successful ingestion receipts.

Uses the existing artifact/provenance validator, never uploads, authenticates, or
changes repository bindings. Execute each prepared case with fresh shared-action
access and symbolicate.py --mode manual-http. Automatic developer GDB is separate.
"""
import argparse
import base64
import copy
import importlib.util
import json
import os
from pathlib import Path
import shutil
import secrets
import subprocess
import tarfile
import zipfile


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


gate = load("symbolicate", "symbolicate.py")
ingest_ci = load("ingest_ci", "ingest-ci.py")
maker = load("symbolication_case", "make-symbolication-case.py")
sqlite_source = load("regenerate_sqlite_source", "regenerate-sqlite-source.py")


def require_receipts(summary, repository):
    if summary["repository"] != repository or summary["status"] != "ingested-and-read-back" or not summary["uploads"]:
        raise ValueError("Require complete same-repository ingestion before address qualification")
    for row in summary["uploads"]:
        if row["readback"].get("sha256") != row["debugSha256"] or row["readback"].get("status") != "exact-bytes":
            raise ValueError("Incomplete exact-byte readback receipt")
    return {(row["debugId"], row["debugSha256"], row["commit"]) for row in summary["uploads"]}


def shipped_file(directory, probe, output):
    candidates = sorted(p for p in directory.glob(probe["distribution"]) if p.is_file())
    if len(candidates) != 1:
        raise ValueError("Reviewed distribution selector must identify one producer artifact member")
    distribution = candidates[0]
    suffix = probe.get("memberSuffix")
    if not suffix:
        shutil.copyfile(distribution, output)
    elif zipfile.is_zipfile(distribution):
        with zipfile.ZipFile(distribution) as archive:
            names = [n for n in archive.namelist() if n.endswith(suffix)]
            if len(names) != 1:
                raise ValueError("Ambiguous/missing shipped ZIP member")
            with archive.open(names[0]) as source, output.open("wb") as target:
                shutil.copyfileobj(source, target)
    else:
        # Stream once, with explicit destinations. No member-controlled paths,
        # symlinks, hardlinks or extraction side effects can enter the workspace.
        found = False
        with tarfile.open(distribution, "r|*") as archive:
            for member in archive:
                if member.isfile() and member.name.endswith(suffix):
                    if found:
                        raise ValueError("Ambiguous shipped tar member")
                    with archive.extractfile(member) as source, output.open("wb") as target:
                        shutil.copyfileobj(source, target)
                    found = True
        if not found:
            raise ValueError("Missing shipped tar member")
    return {"distributionSha256": gate.sha(distribution), "memberSuffix": suffix}


def prepare(config, summary, directory, source_root, nm, foreign_id, output, dependency_root=None, snapshot_root=None):
    repository = os.environ["GITHUB_REPOSITORY"]
    receipts = require_receipts(summary, repository)
    producer = summary["producerRun"]
    inventory_config = copy.deepcopy(config["ingestion"])
    inventory_config["artifacts"] += config["distributionArtifacts"]
    plan = ingest_ci.collect(inventory_config, str(producer["id"]), directory)
    if plan["producerRun"] != producer:
        raise ValueError("Producer run attempt/revision changed; obtain new ingestion receipts")
    output.mkdir(parents=True, exist_ok=False)
    selected, results, distributions = [], [], set()
    for probe in config["probes"]:
        distribution_key = (probe["distribution"], probe.get("memberSuffix"))
        if distribution_key in distributions:
            continue
        distributions.add(distribution_key)
        module = output/("module-"+str(len(selected)))
        provenance = shipped_file(directory, probe, module)
        digest = gate.sha(module)
        uploads = [u for u in plan["uploads"] if any(m["sha256"] == digest for m in u["modules"])]
        if len(uploads) != 1:
            raise ValueError("Shipped bytes do not match one previously ingested final peer")
        upload = uploads[0]
        if (upload["debugId"], upload["debugSha256"], upload["commit"]) not in receipts:
            raise ValueError("Read availability has not been established for this exact new build")
        manifests = [Path(b["manifest"]) for b in plan["builds"] if b["sha256"] in upload["manifestSha256"]]
        if not manifests:
            raise ValueError("No matching final manifest")
        manifest = next((m for m in manifests if any(row["sha256"] == digest for row in json.loads(m.read_text())["modules"])), None)
        if manifest is None:
            raise ValueError("Missing manifest module")
        if probe.get("memberSuffix") and not any(c["sha256"] == provenance["distributionSha256"]
                                                  for c in json.loads(manifest.read_text()).get("containers", [])):
            raise ValueError("Archive bytes differ from the shipped manifest's container provenance")
        provenance["containerVerified"] = bool(probe.get("memberSuffix"))
        retention_path = Path(upload["preservationRecordPath"])
        case = maker.make_case(manifest, module, Path(upload["file"]), config["probes"], source_root, nm, foreign_id,
                               dependency_root, snapshot_root, retention_path)
        retained = output/("retention-"+str(len(selected))+".json")
        shutil.copyfile(retention_path, retained)
        case["distributionEvidence"] = provenance
        case_path = output/("case-"+str(len(selected))+".json")
        case_path.write_text(json.dumps(case, indent=2)+"\n")
        selected.append({"case": str(case_path), "caseSha256": gate.sha(case_path), "module": str(module), "manifest": str(manifest),
                         "sourceRoot": str(source_root), "dependencySourceRoot": str(dependency_root) if dependency_root else None,
                         "sourceSnapshotRoot": str(snapshot_root) if snapshot_root else None,
                         "preservationRecord": str(retained)})
        results.append({"modulePath": case["modulePath"], "moduleSha256": digest, "debugId": case["debugId"],
                        "remoteAvailability": "awaiting-address-gate", "sourceInline": "not-tested"})
    if not selected:
        raise ValueError("No reviewed native address probes; this gate cannot pass empty")
    report = {"repository": repository, "producerRun": producer, "executionNonce": secrets.token_hex(16), "ingestedPeers": len(receipts),
              "gateCodeSha256": gate.gate_code(),
              "sampledModules": len({r["moduleSha256"] for r in results}),
              "unsampledPeers": len(receipts)-len({r["debugId"] for r in results}),
              "cases": selected, "results": results,
              "limits": config["limits"], "retrievalMode": "manual-http; not native automatic GDB proof"}
    (output/"prepared.json").write_text(json.dumps(report, indent=2)+"\n")
    return report


def fetch_sources(summary, source_root, dependency_root, dependencies):
    revisions = {build["commit"] for build in summary["builds"]}
    repository = os.environ["GITHUB_REPOSITORY"]
    remote = subprocess.check_output(["git", "-C", str(source_root), "remote", "get-url", "origin"], text=True).strip().removesuffix(".git")
    if remote != "https://github.com/"+repository:
        raise ValueError("Fetch source only from this repository's GitHub origin")
    env = os.environ.copy()
    token = env.get("GH_TOKEN", "")
    if token:
        env.update({"GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "http.https://github.com/.extraheader",
                    "GIT_CONFIG_VALUE_0": "AUTHORIZATION: basic "+base64.b64encode(("x-access-token:"+token).encode()).decode()})
    for revision in revisions:
        if not gate.re.fullmatch(r"[0-9a-f]{40}", revision):
            raise ValueError("Expected an exact source revision")
        subprocess.run(["git", "-C", str(source_root), "fetch", "--no-tags", "--depth=1", "origin", revision], env=env, check=True)
    if dependencies:
        if len(dependencies) != 1 or dependency_root is None:
            raise ValueError("This sampled gate supports one explicit source dependency")
        path, repository = next(iter(dependencies.items()))
        if repository != "llvm/llvm-project":
            raise ValueError("Only the reviewed public LLVM source dependency is supported")
        links = {subprocess.check_output(["git", "-C", str(source_root), "ls-tree", revision, path], text=True).split()[2]
                 for revision in revisions}
        dependency_root.mkdir(parents=True, exist_ok=False)
        subprocess.run(["git", "init", "-q", str(dependency_root)], check=True)
        subprocess.run(["git", "-C", str(dependency_root), "remote", "add", "origin", "https://github.com/"+repository+".git"], check=True)
        for revision in links:
            subprocess.run(["git", "-C", str(dependency_root), "fetch", "--filter=blob:none", "--depth=1", "origin", revision], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ("config", "summary", "directory", "source-root", "out"):
        parser.add_argument("--"+key, type=Path, required=True)
    parser.add_argument("--dependency-source-root", type=Path)
    parser.add_argument("--source-snapshot-root", type=Path)
    parser.add_argument("--toolchain-manifest", type=Path)
    parser.add_argument("--nm", default="llvm-nm")
    parser.add_argument("--fetch-source", action="store_true")
    parser.add_argument("--foreign-debug-id", required=True)
    args = parser.parse_args()
    config, summary = json.loads(args.config.read_text()), json.loads(args.summary.read_text())
    require_receipts(summary, os.environ["GITHUB_REPOSITORY"])
    if args.fetch_source:
        fetch_sources(summary, args.source_root, args.dependency_source_root, config.get("sourceDependencies", {}))
    if config.get("sourceSnapshots") == "sqlite-jni":
        revisions = {build["commit"] for build in summary["builds"]}
        if len(revisions) != 1 or args.source_snapshot_root is None:
            raise ValueError("One exact Runtime revision and output for owned C source regeneration are required")
        sqlite_source.regenerate(args.source_root, revisions.pop(), args.source_snapshot_root, args.toolchain_manifest)
    report = prepare(config, summary, args.directory,
            args.source_root, args.nm, args.foreign_debug_id, args.out, args.dependency_source_root, args.source_snapshot_root)
    if os.environ.get("GITHUB_OUTPUT"):
        with Path(os.environ["GITHUB_OUTPUT"]).open("a") as stream:
            stream.write("case_count="+str(len(report["cases"]))+"\n")


if __name__ == "__main__":
    main()
