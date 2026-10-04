#!/usr/bin/env python3
"""Release tested artifacts from this workflow run; never rebuild or publish to Central."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
PLATFORMS = {"ubuntu-24.04": "linux-x86_64-gnu", "macos-15": "osx-aarch64"}


def gh(*args):
  return subprocess.check_output(["gh", *args], text=True).strip()


def version():
  return (ROOT / ".version").read_text().strip()


def release_tag():
  value = version()
  return "v" + value if re.fullmatch(r"\d+\.\d+\.\d+", value) else None


def select():
  tag = release_tag()
  if tag is None:
    return
  repo = os.environ["GITHUB_REPOSITORY"]
  # Listing distinguishes an absent draft from API/authentication failures.
  releases = json.loads(gh("api", "--paginate", "--slurp", f"repos/{repo}/releases"))
  draft = next((r for page in releases for r in page if r["tag_name"] == tag), None)
  if draft is None or not draft["draft"]:
    return
  ref = json.loads(gh("api", f"repos/{repo}/git/ref/tags/{tag}"))["object"]
  while ref["type"] == "tag":
    ref = json.loads(gh("api", f"repos/{repo}/git/tags/{ref['sha']}"))["object"]
  if ref["type"] != "commit" or ref["sha"] != os.environ["GITHUB_SHA"]:
    raise RuntimeError("Draft tag does not identify this run's tested commit")
  with open(os.environ["GITHUB_OUTPUT"], "a") as output:
    output.write(f"tag={tag}\n")


def expected_assets(value):
  return {f"dokar-{value}-{platform}-unsigned.zip" for platform in PLATFORMS.values()} | {
      f"provenance-{runner}.json" for runner in PLATFORMS}


def stage():
  source = ROOT / "build/release-input"
  destination = ROOT / "build/release-assets"
  expected = expected_assets(version())
  if {p.name for p in source.iterdir()} != expected:
    raise RuntimeError("Missing or unexpected platform release evidence")
  for runner, platform in PLATFORMS.items():
    gh("attestation", "verify", str(source / f"dokar-{version()}-{platform}-unsigned.zip"),
       "--repo", os.environ["GITHUB_REPOSITORY"],
       "--bundle", str(source / f"provenance-{runner}.json"),
       "--signer-workflow", f"{os.environ['GITHUB_REPOSITORY']}/.github/workflows/job.build.yml",
       "--source-digest", os.environ["GITHUB_SHA"], "--source-ref", "refs/heads/main",
       "--deny-self-hosted-runners")
  shutil.rmtree(destination, ignore_errors=True)
  destination.mkdir(parents=True)
  for name in sorted(expected):
    shutil.copy2(source / name, destination / name)
  checksums = "".join(f"{hashlib.sha256((destination / name).read_bytes()).hexdigest()}  {name}\n"
                      for name in sorted(expected))
  (destination / "SHA256SUMS").write_text(checksums)


def publish():
  repo, tag = os.environ["GITHUB_REPOSITORY"], os.environ["RELEASE_TAG"]
  if tag != release_tag():
    raise RuntimeError("Release tag and source version disagree")
  destination = ROOT / "build/release-assets"
  payload = expected_assets(version()) | {"SHA256SUMS"}
  expected = payload | {name + ".sigstore.json" for name in payload}
  if {p.name for p in destination.iterdir()} != expected:
    raise RuntimeError("Incomplete signed release asset set")
  if gh("release", "view", tag, "--repo", repo, "--json", "isDraft", "--jq", ".isDraft") != "true":
    raise RuntimeError("Refusing to change a published release")
  # Remove obsolete assets only from the draft on a retry. Published releases
  # are never modified. Verify uploaded digests before freezing the release.
  remote = json.loads(gh("release", "view", tag, "--repo", repo, "--json", "assets"))["assets"]
  for asset in remote:
    if asset["name"] not in expected:
      gh("release", "delete-asset", tag, asset["name"], "--yes", "--repo", repo)
  gh("release", "upload", tag, *map(str, sorted(destination.iterdir())), "--clobber", "--repo", repo)
  release = json.loads(gh("api", f"repos/{repo}/releases/tags/{tag}"))
  digests = {a["name"]: a["digest"] for a in release["assets"]}
  for name in expected:
    if digests.get(name) != "sha256:" + hashlib.sha256((destination / name).read_bytes()).hexdigest():
      raise RuntimeError(f"Uploaded asset digest mismatch: {name}")
  if set(digests) != expected:
    raise RuntimeError("Unexpected remote release assets")
  gh("release", "edit", tag, "--draft=false", "--repo", repo)
  if json.loads(gh("api", f"repos/{repo}/releases/tags/{tag}")).get("immutable") is not True:
    raise RuntimeError("Published release is not immutable")


if __name__ == "__main__":
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument("task", choices=("select", "stage", "publish"))
  globals()[parser.parse_args().task]()
