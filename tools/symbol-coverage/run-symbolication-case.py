#!/usr/bin/env python3
"""Execute one prepared sample or enforce complete per-case results. No credentials minted."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def finalize(prepared, output):
    results = []
    for index, entry in enumerate(prepared["cases"]):
        path = output/("case-"+str(index)+".json")
        result = json.loads(path.read_text()) if path.exists() else {"status": "not-run", "outcome": {"integration": "failed", "quality": "failed"}}
        expected = json.loads(Path(entry["case"]).read_text())
        bound = (digest(entry["case"]) == entry["caseSha256"] == result.get("caseSha256")
                 and bool(prepared.get("executionNonce")) and result.get("executionNonce") == prepared["executionNonce"]
                 and all(result.get(key) == expected[key] for key in ("moduleSha256", "debugSha256", "manifestSha256", "debugId", "sourceRevision")))
        result["evidenceBinding"] = "passed" if bound else "failed"
        results.append(result)
    prepared["results"] = results
    prepared["status"] = "passed" if results and all(r["outcome"] == {"integration": "passed", "quality": "passed"}
                                                    and r["evidenceBinding"] == "passed" for r in results) else "failed"
    return prepared


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepared", type=Path, required=True)
    parser.add_argument("--index", type=int)
    parser.add_argument("--finalize", action="store_true")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    prepared = json.loads(args.prepared.read_text())
    args.out.mkdir(parents=True, exist_ok=True)
    if not 1 <= len(prepared["cases"]) <= 3:
        raise ValueError("Expected 1–3 reviewed sample cases")
    if args.finalize:
        prepared = finalize(prepared, args.out)
        (args.out/"summary.json").write_text(json.dumps(prepared, indent=2)+"\n")
        raise SystemExit(0 if prepared["status"] == "passed" else 1)
    if args.index is None or not 0 <= args.index < len(prepared["cases"]):
        raise ValueError("Expected a valid case index")
    case = prepared["cases"][args.index]
    if digest(case["case"]) != case["caseSha256"]:
        raise ValueError("Prepared case changed after source/build binding")
    argv = [sys.executable, str(Path(__file__).with_name("symbolicate.py")), "--mode", "manual-http",
            "--case", case["case"], "--module", case["module"], "--manifest", case["manifest"],
            "--source-root", case["sourceRoot"], "--server", os.environ["DEBUGINFO_DEV_URL"],
            "--execution-nonce", prepared["executionNonce"],
            "--symbolizer", os.environ.get("SYMBOLIZER", "llvm-symbolizer"),
            "--out", str(args.out/("case-"+str(args.index)+".json"))]
    if case.get("dependencySourceRoot"):
        argv += ["--dependency-source-root", case["dependencySourceRoot"]]
    if case.get("sourceSnapshotRoot"):
        argv += ["--source-snapshot-root", case["sourceSnapshotRoot"]]
    if case.get("preservationRecord"):
        argv += ["--preservation-record", case["preservationRecord"]]
    raise SystemExit(subprocess.run(argv).returncode)


if __name__ == "__main__":
    main()
