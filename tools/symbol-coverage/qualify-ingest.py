#!/usr/bin/env python3
"""Live qualification of an already ingested real peer; never creates a fixture identity."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    plan = json.loads(args.plan.read_text())
    if not plan["uploads"]:
        args.out.write_text(json.dumps({"status": "no-supported-peer", "gaps": plan["gaps"]}) + "\n")
        return
    peer = min(plan["uploads"], key=lambda item: Path(item["file"]).stat().st_size)
    prefix = ["dbginfo", "--no-telemetry", "--json", "--url", os.environ["DEBUGINFO_DEV_URL"],
              "--org", os.environ["DEBUGINFO_DEV_ORG"]]
    report = {"debugId": peer["debugId"], "debugSha256": peer["debugSha256"], "checks": {}}
    def command(arguments, expected=0, env=None):
        completed = subprocess.run([*prefix, *arguments], env=env, capture_output=True, text=True)
        if completed.returncode != expected:
            raise RuntimeError(f"Live qualification expected CLI exit {expected}, received {completed.returncode}")
        if expected:
            diagnostic = re.sub(r"https?://\S+", "[service URL]", completed.stderr)
            for name in ("DEBUGINFO_DEV_APIKEY", "GH_TOKEN", "GITHUB_TOKEN"):
                if os.environ.get(name):
                    diagnostic = diagnostic.replace(os.environ[name], "[REDACTED]")
            return {"exitCode": completed.returncode, "diagnostic": diagnostic[:400]}
        return [json.loads(line) for line in completed.stdout.splitlines() if line.strip()]
    identity = command(["whoami"])[0]
    credential = identity["credential"]
    if credential["kind"] != "signed" or credential["scope"] != "WRITE":
        raise ValueError("Live upload controls require the shared action's short-lived WRITE credential")
    report["authenticatedIdentity"] = identity
    metadata = ["--project", os.environ["DEBUGINFO_DEV_PROJECT"], "--repo", plan["repository"],
                "--commit", peer["commit"], "--ref", os.environ["GITHUB_REF"], "--run-id", os.environ["GITHUB_RUN_ID"]]
    original = command(["ingest", peer["file"], *metadata])[0]
    if original["status"] != "exists" or original["debugId"] != peer["debugId"]:
        raise ValueError("Identical real-artifact rerun was not idempotent")
    report["checks"]["idempotent"] = original
    with tempfile.TemporaryDirectory(prefix="qualify-symbol-ingest-") as temporary:
        root = Path(temporary)
        changed = root / "same-identity-different-bytes.debug"
        shutil.copyfile(peer["file"], changed)
        with changed.open("ab") as stream:
            stream.write(b"\nSTEP3_COLLISION_CONTROL\n")
        if command(["id", str(changed)])[0]["debugId"] != peer["debugId"]:
            raise ValueError("Collision control changed the binary identity; no upload attempted")
        report["checks"]["collision"] = command(["ingest", str(changed), *metadata], expected=5)
        for label, secret in (("missingCredential", None), ("invalidCredential", "dbg_w_" + "0" * 48)):
            env = os.environ.copy()
            env["HOME"] = str(root / "empty-home")
            env["XDG_CONFIG_HOME"] = str(root / "empty-config")
            env.pop("DEBUGINFO_DEV_APIKEY", None)
            if secret:
                env["DEBUGINFO_DEV_APIKEY"] = secret
            report["checks"][label] = command(["get", peer["debugId"], "-o", str(root / label)], expected=3, env=env)
        foreign = os.environ.get("DEBUGINFO_DEV_FOREIGN_DEBUG_ID")
        if foreign:
            report["checks"]["foreignPrivateProject"] = command(["get", foreign, "-o", str(root / "foreign")], expected=3)
        original_path = root / "original"
        command(["get", peer["debugId"], "-o", str(original_path)])
        with original_path.open("rb") as stream:
            if hashlib.file_digest(stream, "sha256").hexdigest() != peer["debugSha256"]:
                raise ValueError("Original artifact changed after collision/denial controls")
    report["checks"]["originalUnchanged"] = True
    report["status"] = "passed"
    args.out.write_text(json.dumps(report, indent=2) + "\n")
    print("Real-peer idempotency, collision rejection, denied reads and unchanged readback passed")


if __name__ == "__main__":
    main()
