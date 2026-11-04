#!/usr/bin/env python3
"""Ingest manifest-bound final debug peers using an already authenticated dbginfo.

Authentication belongs to elide-dev/setup-debuginfo. Destination identifiers belong
to each consumer. No directory-wide ingest: maps, diagnostics and supplier files
must never be mistaken for a product's final debug peers.
"""

import argparse
import hashlib
import html
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def bound_file(directory, item):
    path = (directory / item["path"]).resolve()
    if not path.is_relative_to(directory.resolve()):
        raise ValueError("Artifact path escapes its preservation bundle")
    if not path.is_file() or digest(path) != item["sha256"]:
        raise ValueError(f"Retained artifact hash mismatch: {path}")
    return path


def plan(repository, manifests, roots):
    records, final_records = {}, {}
    for root in roots:
        for path in sorted(root.rglob("preservation.json")):
            checksum = digest(path)
            records.setdefault(checksum, []).append(path)
            record = json.loads(path.read_text(encoding="utf-8"))
            if not isinstance(record.get("binary"), dict) or "sha256" not in record["binary"]:
                raise ValueError("Invalid preservation record")
            binary = record["binary"]
            identity = json.dumps(binary.get("identity"), sort_keys=True)
            final_records.setdefault((binary["sha256"], binary.get("architecture"), identity), []).append(
                {"record": record, "evidence": {"sha256": checksum}, "association": "exact-distributed-bytes"})
    uploads, gaps, builds = {}, [], []
    for path in manifests:
        manifest = json.loads(path.read_text(encoding="utf-8"))
        build = manifest["build"]
        if build["repository"] != repository or not re.fullmatch(r"[0-9a-f]{40,64}", build["commit"]):
            raise ValueError("Manifest repository/source commit mismatch")
        if build["trackedWorktreeDirty"] is not False:
            gaps.append({"manifest": str(path), "reason": "Producer recorded a dirty or uninspected worktree; source SHA alone does not identify all build-time changes",
                         "trackedWorktreeDirty": build["trackedWorktreeDirty"]})
        builds.append({"manifest": str(path), "sha256": digest(path), **build})
        for module in manifest["modules"]:
            retained = module["debug"].get("retention", [])
            if not retained:
                # Bazel may emit its distribution inventory before the separate
                # explicit-download preservation action. Join only exact final
                # byte hashes plus architecture/identity, never filenames.
                retained = final_records.get((module.get("sha256"), module.get("architecture"),
                    json.dumps(module.get("identity"), sort_keys=True)), [])
            if not retained:
                gaps.append({"module": module["path"], "reason": "No final preservation record; supplier/ownership not inferred"})
            for retention in retained:
                record = retention["record"]
                if retention["association"] != "exact-distributed-bytes" or record["binary"]["sha256"] != module["sha256"]:
                    raise ValueError(f"Final distributed bytes were not bound: {module['path']}")
                matches = records.get(retention["evidence"]["sha256"], [])
                if not matches:
                    raise ValueError("Missing preservation record referenced by distribution manifest")
                record_path = matches[0]
                if json.loads(record_path.read_text(encoding="utf-8")) != record:
                    raise ValueError("Preservation record differs from manifest")
                directory = record_path.parent
                peer = bound_file(directory, record["debug"])
                for item in record["maps"] + record["dwarf"]["companions"]:
                    bound_file(directory, item)
                diagnostic = record["dwarf"].get("verification", {}).get("diagnostics")
                if diagnostic:
                    bound_file(directory, diagnostic)
                if record["maps"]:
                    gaps.append({"module": module["path"], "reason": "Obfuscation maps retained with hashes; service has no supported deobfuscation-map ingest contract", "maps": record["maps"]})
                if module["format"] not in ("elf", "macho") or record["dwarf"]["companions"]:
                    gaps.append({"module": module["path"], "reason": "Standalone PDB/split-DWARF companion ingest is unsupported", "debug": record["debug"]})
                    continue
                identity = module["identity"]["value"]
                if not re.fullmatch(r"[0-9a-f]{8,128}", identity):
                    raise ValueError("Expected a supported single-image debug identity")
                key = (identity, record["debug"]["sha256"], build["commit"])
                if any(k[0] == identity and k[1] != key[1] for k in uploads):
                    raise ValueError("Different debug bytes under one identity")
                item = uploads.setdefault(key, {"file": str(peer), "debugId": identity, "debugSha256": key[1],
                    "commit": build["commit"], "modules": [], "preservationSha256": digest(record_path),
                    "preservationRecordPath": str(record_path),
                    "dwarf": record["dwarf"], "maps": record["maps"], "manifestSha256": []})
                if digest(path) not in item["manifestSha256"]:
                    item["manifestSha256"].append(digest(path))
                item["modules"].append({"path": module["path"], "sha256": module["sha256"], "architecture": module["architecture"]})
        gaps.extend(manifest["gaps"])
    if not uploads and not gaps:
        raise ValueError("Empty ingestion plan")
    return {"version": 1, "repository": repository, "builds": builds, "uploads": list(uploads.values()), "gaps": gaps}


def safe_detail(detail):
    if "<html" in detail.lower():
        detail = re.sub(r"(?is)<(script|style)\b[^>]*>.*?</\1>", " ", detail)
        detail = html.unescape(re.sub(r"<[^>]+>", " ", detail))
        detail = " ".join(detail.split())
    detail = re.sub(r"https?://\S+", "[service URL]", detail)
    for name in ("DEBUGINFO_DEV_APIKEY", "GH_TOKEN", "GITHUB_TOKEN"):
        if os.environ.get(name):
            detail = detail.replace(os.environ[name], "[REDACTED]")
    return detail[:1600]


def cli(*args):
    result = subprocess.run(["dbginfo", "--no-telemetry", "--json", *args], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(f"dbginfo failed (exit {result.returncode}); ingestion incomplete: {safe_detail(result.stderr)}")
    return [json.loads(line) for line in result.stdout.splitlines() if line.strip()]


def download(url, org, debug_id, path):
    """Independent debuginfod GET using the shared action's existing credential.

    curl streams to disk with a connection/idle deadline rather than a deadline
    for the entire file. No credential exchange or signing is implemented here.
    """
    key = os.environ.get("DEBUGINFO_DEV_APIKEY", "")
    if not url.startswith("https://") or not re.fullmatch(r"[0-9a-f]{8,128}", debug_id):
        raise ValueError("Expected an HTTPS debuginfod URL and canonical debug identity")
    if not key or any(c in key + org for c in "\r\n"):
        raise ValueError("Expected the shared action's credential and a single-line tenant")
    # stdin keeps the bearer credential out of argv and process listings. Do
    # not follow redirects or load ~/.curlrc: these bytes must come from the
    # configured service, and the credential must not leave that origin.
    config = "\n".join("header = " + json.dumps(value) for value in (
        "Authorization: Bearer " + key, "X-Debuginfo-Tenant: " + org)) + "\n"
    started = time.monotonic()
    for attempt in range(3):
        Path(path).unlink(missing_ok=True)
        result = subprocess.run(["curl", "--disable", "--fail", "--silent", "--show-error",
            "--proto", "=https", "--connect-timeout", "30", "--speed-limit", "1", "--speed-time", "30",
            "--config", "-", "--output", str(path), f"{url.rstrip('/')}/buildid/{debug_id}/debuginfo"],
            input=config, capture_output=True, text=True)
        if not result.returncode:
            return
        Path(path).unlink(missing_ok=True)
        # Retry only interrupted GET transports: partial body, timeout, empty
        # reply, send/receive failure or HTTP/2 stream reset. HTTP errors (22),
        # including auth denials, remain terminal. No upload is repeated here.
        delay = 2 ** attempt
        if result.returncode not in (18, 28, 52, 55, 56, 92) or attempt == 2 or time.monotonic() - started + delay >= 180:
            break
        print(f"Interrupted debuginfod GET (curl {result.returncode}); retry {attempt + 1}/2", flush=True)
        time.sleep(delay)
    raise RuntimeError(f"debuginfod readback failed (curl exit {result.returncode}); ingestion incomplete: {safe_detail(result.stderr)}")


def private_project(url, org, org_id, project, project_id):
    if not os.environ.get("DEBUGINFO_DEV_APIKEY"):
        raise ValueError("Run the shared setup-debuginfo action immediately before ingest")
    identity = cli("--url", url, "--org", org, "whoami")
    if len(identity) != 1 or identity[0].get("orgId") != org_id or identity[0].get("org") != org:
        raise ValueError("Configured organization does not match the authenticated service identity")
    projects = cli("--url", url, "--org", org, "projects", "ls")
    for item in projects:
        if item["id"] == project_id and item["slug"] == project:
            if item["visibility"] != "private":
                raise ValueError("Destination must remain PRIVATE")
            return {**item, "orgId": org_id}
    raise ValueError("Configured destination project was not found")


def upload(result, destination, ref, receipt_path):
    url, org, org_id, project, project_id = [destination[k] for k in ("url", "org", "orgId", "project", "projectId")]
    if not url.startswith("https://") or not project_id or not ref:
        raise ValueError("Explicit HTTPS/private project/ref configuration required")
    result["destination"] = private_project(url, org, org_id, project, project_id)
    result["destination"]["url"] = url
    result["workflow"] = {k: os.environ.get(k) for k in ("GITHUB_SHA", "GITHUB_REF", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT", "GITHUB_WORKFLOW_REF")}
    # The service correctly trusts the signed workflow driver SHA over caller
    # headers. Preserve the actual (possibly older tag) producer SHA separately.
    result["serviceBuild"] = {"commit": os.environ.get("GITHUB_SHA"), "ref": os.environ.get("GITHUB_REF"),
        "runId": os.environ.get("GITHUB_RUN_ID", "") + "-" + os.environ.get("GITHUB_RUN_ATTEMPT", "1"),
        "sourceCommitContract": "actual producer commit and manifest digest retained in displayArchive and this receipt; signed service Build identifies upload workflow"}
    run = os.environ.get("GITHUB_RUN_ID")
    if not run or os.environ.get("GITHUB_REPOSITORY") != result["repository"]:
        raise ValueError("Expected the repository's own GitHub Actions run")
    result["status"] = "incomplete"
    receipt_path.parent.mkdir(parents=True, exist_ok=True)
    def save():
        receipt_path.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    save()
    for item in result["uploads"]:
        print(f"Ingesting {item['debugId']} ({Path(item['file']).stat().st_size} bytes)", flush=True)
        if digest(item["file"]) != item["debugSha256"]:
            raise ValueError("Debug bytes changed after planning")
        outcome = cli("--url", url, "--org", org, "ingest", item["file"], "--project", project,
            "--repo", result["repository"], "--commit", item["commit"], "--ref", ref, "--run-id", run,
            "--display-path", item["modules"][0]["path"],
            "--archive", f"source-commit:{item['commit']};manifest-sha256:{item['manifestSha256'][0]}")
        if len(outcome) != 1 or outcome[0].get("debugId") != item["debugId"] or not outcome[0].get("artifactId"):
            raise ValueError("Missing or mismatched server identity/receipt")
        item["receipt"] = outcome[0]
        save()
        with tempfile.TemporaryDirectory(prefix="symbol-readback-") as temporary:
            path = Path(temporary) / "debug"
            download(url, org, item["debugId"], path)
            if digest(path) != item["debugSha256"]:
                raise ValueError("Server readback SHA-256 mismatch")
        item["readback"] = {"sha256": item["debugSha256"], "status": "exact-bytes", "client": "curl", "protocol": "debuginfod"}
        save()
    result["status"] = "ingested-and-read-back" if result["uploads"] else "unsupported-format-gaps-only"
    save()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--manifest", type=Path, action="append", required=True)
    parser.add_argument("--symbols", type=Path, action="append", required=True)
    parser.add_argument("--destination", type=Path)
    parser.add_argument("--ref", default=os.environ.get("GITHUB_REF"))
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--upload", action="store_true")
    args = parser.parse_args()
    result = plan(args.repository, args.manifest, args.symbols)
    if args.upload:
        if not args.destination:
            parser.error("--upload requires a repository-owned --destination config")
        upload(result, json.loads(args.destination.read_text(encoding="utf-8")), args.ref, args.out)
    else:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(f"{len(result['uploads'])} final debug peers; {len(result['gaps'])} explicit gaps; {args.out}")


if __name__ == "__main__":
    main()
