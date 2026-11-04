#!/usr/bin/env python3
"""Consumer CI adapter: select exact tested build artifacts, then invoke ingest.py.

The consumer owns artifact names/destinations and calls the shared identity action.
GitHub tokens are used by gh only; symbol authentication is never implemented here.
"""

import argparse
import copy
import fnmatch
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile

spec = importlib.util.spec_from_file_location("symbol_ingest", Path(__file__).with_name("ingest.py"))
ingest = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ingest)


def gh(*args):
    result = subprocess.run(["gh", *args], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError("GitHub artifact lookup/download failed; ingestion incomplete")
    return result.stdout


def batches(uploads, limit=2 * 1024 ** 3):
    result, current, size = [], [], 0
    for item in uploads:
        length = Path(item["file"]).stat().st_size
        if current and size + length > limit:
            result.append(current)
            current, size = [], 0
        current.append(item)
        size += length
    if current:
        result.append(current)
    if len(result) > 32:
        raise ValueError("More than 32 credential batches required; split the consumer platform/artifact selection")
    return result


def collect(config, source_run, directory):
    if not str(source_run).isdecimal():
        raise ValueError("Expected a numeric producer workflow run ID")
    repository = os.environ["GITHUB_REPOSITORY"]
    platform = os.environ.get("SYMBOL_INGEST_PLATFORM", "all")
    patterns = [pattern.format(platform=platform) for pattern in config["artifacts"]]
    run = json.loads(gh("api", f"repos/{repository}/actions/runs/{source_run}"))
    if run["repository"]["full_name"] != repository or run["head_repository"]["full_name"] != repository:
        raise ValueError("Only this repository's own build artifacts are accepted")
    if run["event"] in ("pull_request", "pull_request_target"):
        raise ValueError("Private ingest does not accept PR artifacts")
    branch = run["head_branch"]
    if branch not in config["trustedBranches"]:
        if not any(fnmatch.fnmatchcase(branch, ref) for ref in config.get("trustedTags", [])):
            raise ValueError("Producer ref is outside this repository's explicitly trusted refs")
        tag = json.loads(gh("api", f"repos/{repository}/git/ref/tags/{branch}"))["object"]
        for _ in range(8):
            if tag["type"] != "tag":
                break
            tag = json.loads(gh("api", f"repos/{repository}/git/tags/{tag['sha']}"))["object"]
        if tag["type"] != "commit" or tag["sha"] != run["head_sha"]:
            raise ValueError("Release tag does not identify the tested producer commit")
    if str(source_run) != os.environ["GITHUB_RUN_ID"] and run["conclusion"] != "success":
        raise ValueError("A reused producer run must have completed successfully")
    pages = json.loads(gh("api", "--paginate", "--slurp", f"repos/{repository}/actions/runs/{source_run}/artifacts?per_page=100"))
    available = [a for page in pages for a in page["artifacts"]]
    artifacts = [a for a in available
                 if any(fnmatch.fnmatchcase(a["name"], pattern) for pattern in patterns)]
    if not any(a["name"].startswith("symbol-ingest-manifest-") for a in artifacts):
        artifacts += [a for a in available if any(fnmatch.fnmatchcase(a["name"], pattern)
                     for pattern in config.get("fallbackArtifacts", []))]
    if not artifacts or any(a["expired"] for a in artifacts):
        raise ValueError("Missing/expired final symbol or distribution evidence")
    directory.mkdir(parents=True, exist_ok=False)
    for artifact in artifacts:
        gh("run", "download", str(source_run), "-R", repository, "-n", artifact["name"], "-D", str(directory / str(artifact["id"])))
    # Komodo's producer retains its peers as a tar, alongside its final archive.
    # Extract only explicitly configured private-symbol archives, using Python's
    # restrictive data filter; never unpack unrelated supplier archives here.
    for pattern in config.get("symbolArchives", []):
        for index, path in enumerate(sorted(directory.glob(pattern))):
            target = directory / ("expanded-" + str(index) + "-" + path.name)
            target.mkdir()
            with tarfile.open(path) as archive:
                archive.extractall(target, filter="data")
    manifests = sorted({p for pattern in config["manifests"] for p in directory.glob(pattern) if p.is_file()})
    if not manifests:
        raise ValueError("Final distribution manifests missing; no upload attempted")
    result = ingest.plan(repository, manifests, [directory])
    evidence = directory / "evidence"
    evidence.mkdir()
    for path in manifests:
        shutil.copyfile(path, evidence / (ingest.digest(path) + ".manifest.json"))
    for item in result["uploads"]:
        path = Path(item["preservationRecordPath"])
        target = evidence / item["preservationSha256"]
        target.mkdir(exist_ok=True)
        shutil.copyfile(path, target / "preservation.json")
        record = json.loads(path.read_text())
        retained = record["maps"] + record["dwarf"]["companions"]
        diagnostic = record["dwarf"].get("verification", {}).get("diagnostics")
        for artifact in retained + ([diagnostic] if diagnostic else []):
            original = ingest.bound_file(path.parent, artifact)
            output = target / artifact["path"]
            output.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(original, output)
    result["producerRun"] = {k: run[k] for k in ("id", "head_sha", "head_branch", "event", "html_url", "run_attempt")}
    result["producerArtifacts"] = [{k: a.get(k) for k in ("id", "name", "digest", "size_in_bytes")} for a in artifacts]
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--source-run", default=os.environ.get("GITHUB_RUN_ID"))
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--upload", action="store_true")
    parser.add_argument("--execute-plan", action="store_true")
    parser.add_argument("--batch", type=int)
    parser.add_argument("--receipts", type=Path)
    parser.add_argument("--finalize", action="store_true")
    args = parser.parse_args()
    config = json.loads(args.config.read_text())
    result = json.loads(args.out.read_text()) if args.execute_plan or args.finalize else collect(config, args.source_run, args.directory)
    if result["repository"] != os.environ["GITHUB_REPOSITORY"]:
        raise ValueError("Ingestion plan belongs to a different repository")
    if args.finalize:
        if not args.receipts:
            parser.error("--finalize requires --receipts")
        verified = {}
        for path in args.receipts.glob("batch-*.json"):
            receipt = json.loads(path.read_text())
            if receipt["status"] != "ingested-and-read-back" or receipt["repository"] != result["repository"]:
                raise ValueError("Incomplete or foreign batch receipt")
            for item in receipt["uploads"]:
                verified[(item["debugId"], item["debugSha256"], item["commit"])] = item
        for index, item in enumerate(result["uploads"]):
            result["uploads"][index] = verified[(item["debugId"], item["debugSha256"], item["commit"])]
        result["status"] = "ingested-and-read-back" if result["uploads"] else "unsupported-format-gaps-only"
        (args.receipts / "summary.json").write_text(json.dumps(result, indent=2) + "\n")
        print(result["status"])
        return
    if not args.execute_plan:
        result["status"] = "planned" if result["uploads"] else "unsupported-format-gaps-only"
        result["batchCount"] = len(batches(result["uploads"]))
        if os.environ.get("GITHUB_OUTPUT"):
            with open(os.environ["GITHUB_OUTPUT"], "a") as output:
                output.write("has_uploads=" + str(bool(result["uploads"])).lower() + "\n")
                output.write("batch_count=" + str(result["batchCount"]) + "\n")
        if os.environ.get("GITHUB_STEP_SUMMARY"):
            with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
                summary.write(f"Final symbol evidence: {len(result['uploads'])} supported peers selected; {len(result['gaps'])} explicit gaps retained in receipt.\n")
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(result, indent=2) + "\n")
    if args.upload:
        if args.batch is None or not args.receipts:
            parser.error("--upload requires --batch and --receipts")
        batch = copy.deepcopy(result)
        batch["uploads"] = batches(result["uploads"])[args.batch]
        destination = {k: os.environ[variable] for k, variable in {
            "url": "DEBUGINFO_DEV_URL", "org": "DEBUGINFO_DEV_ORG", "orgId": "DEBUGINFO_DEV_ORG_ID",
            "project": "DEBUGINFO_DEV_PROJECT", "projectId": "DEBUGINFO_DEV_PROJECT_ID"}.items()}
        ingest.upload(batch, destination, os.environ["GITHUB_REF"], args.receipts / f"batch-{args.batch}.json")
    print(f"{len(result['uploads'])} selected final peers; {len(result['gaps'])} explicit gaps")


if __name__ == "__main__":
    main()
