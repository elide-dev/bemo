#!/usr/bin/env python3
"""Retain bounded ingestion metadata; large diagnostics remain with their producer."""
import argparse
import hashlib
import json
import os
import stat
from pathlib import Path
import tempfile

MAX_TOTAL = 64 * 1024 * 1024
MAX_FILE = 16 * 1024 * 1024
MAX_FILES = 4096
MAX_ENTRIES = 20000


def prepare(runner, output):
  runner = Path(runner).resolve(strict=True)
  output = Path(output)
  if output.parent.resolve() != runner or output.exists() or output.is_symlink():
    raise ValueError("A fresh output directory directly under runner temp is required")
  if output.name in {"ingest-input", "ingest-receipts", "ingest-plan.json"}:
    raise ValueError("Output must not replace ingestion inputs")
  stage = Path(tempfile.mkdtemp(prefix=".ingest-records-", dir=runner))
  files = []
  excluded_count = 0
  excluded_bytes = 0
  total = 0
  entries = 0
  roots = [runner / "ingest-plan.json", runner / "ingest-receipts",
           runner / "ingest-input" / "evidence"]
  for root in roots:
    for parent in (root, *root.parents):
      if parent == runner:
        break
      if parent.is_symlink():
        raise ValueError("Symlinked metadata root")
    if not root.exists():
      continue
    candidates = [root] if root.is_file() else root.rglob("*")
    for source in candidates:
      entries += 1
      if entries > MAX_ENTRIES:
        raise ValueError("Metadata directory exceeds entry budget")
      if source.is_symlink() or not source.resolve().is_relative_to(runner):
        raise ValueError("Metadata path escapes runner temp")
      if source.is_dir():
        continue
      before = source.stat()
      if not stat.S_ISREG(before.st_mode):
        raise ValueError("Metadata must be a regular file")
      if source.suffix != ".json":
        excluded_count += 1
        excluded_bytes += before.st_size
        continue
      if len(files) >= MAX_FILES or before.st_size > MAX_FILE or total + before.st_size > MAX_TOTAL:
        raise ValueError("Ingestion metadata exceeds its explicit byte/file budget")
      relative = source.relative_to(runner)
      destination = stage / relative
      destination.parent.mkdir(parents=True, exist_ok=True)
      digest = hashlib.sha256()
      copied = 0
      with source.open("rb") as src, destination.open("xb") as dst:
        while block := src.read(65536):
          copied += len(block)
          if copied > before.st_size:
            raise ValueError("Metadata grew during capture")
          digest.update(block)
          dst.write(block)
      after = source.stat()
      if copied != before.st_size or (before.st_ino, before.st_size, before.st_mtime_ns) != (after.st_ino, after.st_size, after.st_mtime_ns):
        raise ValueError("Metadata changed during capture")
      total += copied
      files.append({"path": relative.as_posix(), "bytes": copied, "sha256": digest.hexdigest()})
  document = json.dumps({"version": 1, "files": files, "metadataBytes": total,
                         "excludedFiles": excluded_count, "excludedBytes": excluded_bytes,
                         "note": "Only JSON records are copied. Original ingestion inputs are untouched. Large diagnostics must be retained with the producer named in the ingestion plan."}, indent=2).encode()
  if total + len(document) > MAX_TOTAL:
    raise ValueError("Metadata index exceeds total byte budget")
  (stage / "artifact-index.json").write_bytes(document)
  os.rename(stage, output)
  return {"files": len(files), "bytes": total + len(document), "excludedBytes": excluded_bytes}


if __name__ == "__main__":
  parser = argparse.ArgumentParser()
  parser.add_argument("--runner-temp", required=True)
  parser.add_argument("--output", required=True)
  args = parser.parse_args()
  print(json.dumps(prepare(args.runner_temp, args.output)))
