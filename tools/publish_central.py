#!/usr/bin/env python3
"""Sign a merged release bundle, then separately validate or publish it through Central."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time
import urllib.parse
import urllib.request
import uuid
import zipfile

import build
import publish_packages as packages

API = "https://central.sonatype.com/api/v1/publisher"


def release_version(value):
  if not re.fullmatch(r"\d+\.\d+\.\d+", value):
    raise ValueError("Central requires a stable release version (no snapshots)")
  return value


def prepare(source, destination, value, key, *, include_thinlto=True):
  """Merge checked platform artifacts and sign without any Central credentials."""
  release_version(value)
  destination.parent.mkdir(parents=True, exist_ok=True)
  with tempfile.TemporaryDirectory(prefix="bemo-central-") as directory:
    repository = packages.merge(source, Path(directory) / "maven", value, include_thinlto=include_thinlto)
    artifacts = sorted(path for path in repository.rglob("*") if path.is_file())
    for path in artifacts:
      signature = Path(str(path) + ".asc")
      command = ["gpg", "--batch", "--armor", "--detach-sign", "--local-user", key,
                 "--output", str(signature), str(path)]
      passphrase = os.environ.get("BEMO_PGP_PASSPHRASE")
      if passphrase is not None:
        command[1:1] = ["--pinentry-mode", "loopback", "--passphrase-fd", "0"]
      subprocess.run(command, input=passphrase.encode() if passphrase is not None else None, check=True)
      for algorithm in packages.CHECKSUMS:
        Path(str(path) + "." + algorithm).write_text(
            hashlib.new(algorithm, path.read_bytes()).hexdigest() + "\n")
    bundle = Path(directory) / "bundle.zip"
    with zipfile.ZipFile(bundle, "w", zipfile.ZIP_DEFLATED) as archive:
      for path in sorted(repository.rglob("*")):
        if path.is_file():
          archive.write(path, path.relative_to(repository).as_posix())
    validate_bundle(bundle, value)
    # Existing output survives any merge/signing/validation failure.
    temporary = destination.with_suffix(destination.suffix + ".tmp")
    temporary.write_bytes(bundle.read_bytes())
    temporary.replace(destination)
  return destination


def validate_bundle(bundle, value):
  release_version(value)
  with zipfile.ZipFile(bundle) as archive:
    names = archive.namelist()
    include_thinlto = any(name.endswith("-thinlto.jar") for name in names)
    required = set().union(*(packages.artifacts(value, platform, include_thinlto=include_thinlto)
                            for platform in packages.release.PLATFORMS.values()))
    expected = required | {name + ".asc" for name in required} | {
        name + "." + algorithm for name in required for algorithm in packages.CHECKSUMS}
    if len(names) != len(set(names)) or set(names) != expected:
      raise RuntimeError("Incomplete or unexpected Central bundle entries")
    for name in required:
      content = archive.read(name)
      for algorithm in packages.CHECKSUMS:
        if archive.read(name + "." + algorithm).decode().strip() != hashlib.new(algorithm, content).hexdigest():
          raise RuntimeError(f"Invalid {algorithm} checksum: {name}")
      with tempfile.TemporaryDirectory(prefix="bemo-signature-") as directory:
        path = Path(directory) / "artifact"
        path.write_bytes(content)
        signature = Path(directory) / "artifact.asc"
        signature.write_bytes(archive.read(name + ".asc"))
        subprocess.run(["gpg", "--batch", "--quiet", "--verify", str(signature), str(path)], check=True)


def fetch_release(destination, value):
  """Retrieve and authenticate existing immutable release bits; never rebuild."""
  release_version(value)
  repo = "elide-dev/bemo"
  tag = "v" + value
  release = json.loads(packages.release.gh("api", f"repos/{repo}/releases/tags/{tag}"))
  if release.get("draft") or release.get("prerelease") or release.get("immutable") is not True:
    raise RuntimeError("Central requires a published immutable stable GitHub release")
  obj = json.loads(packages.release.gh("api", f"repos/{repo}/git/ref/tags/{tag}"))["object"]
  while obj["type"] == "tag":
    obj = json.loads(packages.release.gh("api", f"repos/{repo}/git/tags/{obj['sha']}"))["object"]
  if obj["type"] != "commit" or not re.fullmatch(r"[0-9a-f]{40}", obj["sha"]):
    raise RuntimeError("Release tag does not resolve to a source commit")
  commit = obj["sha"]
  payload = packages.release.expected_assets(value)
  names = payload | {"SHA256SUMS"}
  names |= {name + ".sigstore.json" for name in names}
  assets = {asset["name"]: asset for asset in release["assets"]}
  if not names.issubset(assets):
    raise RuntimeError("GitHub release is missing signed platform artifacts")
  destination.parent.mkdir(parents=True, exist_ok=True)
  with tempfile.TemporaryDirectory(prefix="bemo-release-") as directory:
    downloaded = Path(directory)
    command = ["gh", "release", "download", tag, "--repo", repo, "--dir", str(downloaded)]
    for name in sorted(names):
      command.extend(("--pattern", name))
    subprocess.run(command, check=True)
    for name in names:
      digest = "sha256:" + hashlib.sha256((downloaded / name).read_bytes()).hexdigest()
      if assets[name].get("digest") != digest:
        raise RuntimeError("GitHub release digest mismatch: " + name)
    subprocess.run(["cosign", "verify-blob", str(downloaded / "SHA256SUMS"),
                    "--bundle", str(downloaded / "SHA256SUMS.sigstore.json"),
                    "--certificate-identity", f"https://github.com/{repo}/.github/workflows/job.release.yml@refs/heads/main",
                    "--certificate-oidc-issuer", "https://token.actions.githubusercontent.com"], check=True)
    checksums = {}
    for line in (downloaded / "SHA256SUMS").read_text().splitlines():
      digest, name = line.split()
      if name not in payload or name in checksums:
        raise RuntimeError("Unexpected or duplicate signed release checksum")
      checksums[name] = digest
    if set(checksums) != payload:
      raise RuntimeError("Incomplete signed release checksums")
    for name, digest in checksums.items():
      if digest != hashlib.sha256((downloaded / name).read_bytes()).hexdigest():
        raise RuntimeError("Signed release checksum mismatch: " + name)
    for runner, platform in packages.release.PLATFORMS.items():
      subprocess.run(["gh", "attestation", "verify", str(downloaded / f"bemo-{value}-{platform}-unsigned.zip"),
                      "--repo", repo, "--bundle", str(downloaded / f"provenance-{runner}.json"),
                      "--signer-workflow", f"{repo}/.github/workflows/job.build.yml",
                      "--source-digest", commit, "--source-ref", "refs/heads/main",
                      "--deny-self-hosted-runners"], check=True)
    destination.mkdir(parents=True, exist_ok=True)
    for name in sorted(names):
      shutil.copy2(downloaded / name, destination / name)
    manifest = {"version": value, "tag": tag, "source_commit": commit,
                "release_url": release["html_url"], "sha256": checksums}
    (destination / "central-release.json").write_text(json.dumps(manifest, indent=2) + "\n")
  return manifest


def request(path, data=b"", content_type="application/json"):
  username = os.environ["CENTRAL_TOKEN_USERNAME"]
  password = os.environ["CENTRAL_TOKEN_PASSWORD"]
  if not username or not password:
    raise RuntimeError("Central Portal username and password must both be configured")
  token = base64.b64encode(f"{username}:{password}".encode()).decode()
  req = urllib.request.Request(API + path, data=data, method="POST", headers={
      "Authorization": "Bearer " + token, "Content-Type": content_type})
  with urllib.request.urlopen(req, timeout=120) as response:
    return response.read().decode()


def upload(bundle, value):
  validate_bundle(bundle, value)
  boundary = "bemo-" + uuid.uuid4().hex
  data = (f'--{boundary}\r\nContent-Disposition: form-data; name="bundle"; filename="bundle.zip"\r\n'
          'Content-Type: application/octet-stream\r\n\r\n').encode()
  data += bundle.read_bytes() + f"\r\n--{boundary}--\r\n".encode()
  query = urllib.parse.urlencode({"name": "bemo-" + value, "publishingType": "USER_MANAGED"})
  deployment = request("/upload?" + query, data, "multipart/form-data; boundary=" + boundary).strip()
  return str(uuid.UUID(deployment))


def status(deployment):
  deployment = str(uuid.UUID(deployment))
  return json.loads(request("/status?" + urllib.parse.urlencode({"id": deployment})))


def publish(deployment):
  deployment = str(uuid.UUID(deployment))
  state = status(deployment)
  if state.get("deploymentState") != "VALIDATED":
    raise RuntimeError("Central deployment must be VALIDATED before publishing: " + json.dumps(state))
  request("/deployment/" + deployment)


def wait(deployment, target, timeout=1800, interval=15):
  """Wait for the Portal state, reporting failure instead of treating POST as completion."""
  if target not in ("VALIDATED", "PUBLISHED") or timeout <= 0 or interval <= 0:
    raise ValueError("Expected a valid target state and positive polling limits")
  deadline = time.monotonic() + timeout
  while True:
    result = status(deployment)
    state = result.get("deploymentState")
    print("Central deployment state: " + str(state), flush=True)
    if state == target or state == "PUBLISHED":
      return result
    if state == "FAILED":
      raise RuntimeError("Central validation failed: " + json.dumps(result.get("errors", result)))
    if state not in ("PENDING", "VALIDATING", "VALIDATED", "PUBLISHING"):
      raise RuntimeError("Unexpected Central deployment state: " + str(state))
    if target == "PUBLISHED" and state == "VALIDATED":
      raise RuntimeError("Deployment is validated but publication has not been requested")
    remaining = deadline - time.monotonic()
    if remaining <= 0:
      raise TimeoutError("Timed out waiting for Central " + target + ": " + deployment)
    time.sleep(min(interval, remaining))


def verify_remote(bundle, value):
  """Compare each published POM/JAR to the signed, locally reviewed bundle."""
  validate_bundle(bundle, value)
  with zipfile.ZipFile(bundle) as archive:
    for name in sorted(archive.namelist()):
      if name.endswith((".pom", ".jar")):
        actual = packages.read_remote("https://repo.maven.apache.org/maven2/" + name, {})
        if actual != archive.read(name):
          raise RuntimeError("Central artifact differs from signed bundle: " + name)


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument("task", choices=("fetch", "prepare", "upload", "status", "wait", "publish", "verify"))
  parser.add_argument("--version", default=build.VERSION)
  parser.add_argument("--source", type=Path, default=build.BUILD / "release-assets")
  parser.add_argument("--bundle", type=Path)
  parser.add_argument("--key", help="PGP signing key fingerprint (prepare only)")
  parser.add_argument("--without-thinlto", action="store_true", help="Prepare an older release with only ordinary native classifiers")
  parser.add_argument("--state", choices=("VALIDATED", "PUBLISHED"), default="VALIDATED")
  parser.add_argument("--timeout", type=int, default=1800)
  parser.add_argument("--deployment", help="Central deployment UUID (status/publish)")
  args = parser.parse_args()
  bundle = args.bundle or build.BUILD / "central" / f"bemo-{args.version}-central.zip"
  if args.task == "fetch":
    print(json.dumps(fetch_release(args.source, args.version), indent=2))
  elif args.task == "prepare":
    if not args.key:
      parser.error("prepare requires --key")
    print(prepare(args.source, bundle, args.version, args.key, include_thinlto=not args.without_thinlto))
  elif args.task == "upload":
    print(upload(bundle, args.version))
  elif args.task == "verify":
    verify_remote(bundle, args.version)
    print("Central artifacts match the signed bundle")
  else:
    if not args.deployment:
      parser.error("status/wait/publish requires --deployment")
    if args.task == "status":
      print(json.dumps(status(args.deployment), indent=2))
    elif args.task == "wait":
      print(json.dumps(wait(args.deployment, args.state, args.timeout), indent=2))
    else:
      publish(args.deployment)
      print("Central publication requested: " + args.deployment)


if __name__ == "__main__":
  main()
