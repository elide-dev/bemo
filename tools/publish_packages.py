#!/usr/bin/env python3
"""Merge verified platform bundles and deploy their existing Maven artifacts."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import xml.etree.ElementTree as ET
import zipfile

import build
import release

MODULES = ("api", "ffm", "netty", "native-image")
CHECKSUMS = ("md5", "sha1", "sha256", "sha512")
REGISTRY = "https://maven.pkg.github.com/elide-dev/bemo"
DEPLOY_GOAL = "org.apache.maven.plugins:maven-deploy-plugin:3.2.0:deploy-file"


def artifacts(value, platform=None):
  names = set()
  for module in MODULES:
    prefix = f"dev/elide/bemo/bemo-{module}/{value}/bemo-{module}-{value}"
    names.update(prefix + suffix for suffix in (".pom", ".jar", "-sources.jar", "-javadoc.jar"))
    if platform and module in ("ffm", "native-image"):
      names.add(f"{prefix}-{platform}.jar")
  return names


def merge(source, destination, value):
  merged = {}
  for platform in release.PLATFORMS.values():
    bundle = source / f"bemo-{value}-{platform}-unsigned.zip"
    required = artifacts(value, platform)
    expected = required | {f"{name}.{algorithm}" for name in required for algorithm in CHECKSUMS}
    with zipfile.ZipFile(bundle) as archive:
      names = archive.namelist()
      if len(names) != len(set(names)) or set(names) != expected:
        raise RuntimeError(f"Missing, duplicate, or unexpected Maven entries in {bundle.name}")
      for name in sorted(required):
        content = archive.read(name)
        for algorithm in CHECKSUMS:
          if archive.read(f"{name}.{algorithm}").decode().strip() != hashlib.new(algorithm, content).hexdigest():
            raise RuntimeError(f"Invalid {algorithm} checksum: {name}")
        if name in merged and merged[name] != content:
          raise RuntimeError(f"Common Maven artifacts differ between platforms: {name}")
        merged[name] = content
  for module in MODULES:
    name = f"dev/elide/bemo/bemo-{module}/{value}/bemo-{module}-{value}.pom"
    pom = ET.fromstring(merged[name])
    ns = {"m": "http://maven.apache.org/POM/4.0.0"}
    for field, expected in (("groupId", "dev.elide.bemo"), ("artifactId", f"bemo-{module}"),
                            ("version", value), ("packaging", "jar")):
      if pom.findtext(f"m:{field}", namespaces=ns) != expected:
        raise RuntimeError(f"Unexpected Maven coordinates in {name}")
  shutil.rmtree(destination, ignore_errors=True)
  for name, content in sorted(merged.items()):
    path = destination / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)
  return destination


def deploy(repository, value, settings, url=REGISTRY):
  for module in MODULES:
    prefix = repository / f"dev/elide/bemo/bemo-{module}/{value}/bemo-{module}-{value}"
    classifiers = ["sources", "javadoc"]
    if module in ("ffm", "native-image"):
      classifiers += sorted(release.PLATFORMS.values())
    command = ["mvn", "--batch-mode", "--no-transfer-progress", "--settings", str(settings), DEPLOY_GOAL,
               "-DrepositoryId=github", f"-Durl={url}", f"-DpomFile={prefix}.pom", f"-Dfile={prefix}.jar",
               "-DgeneratePom=false", "-DretryFailedDeploymentCount=3",
               "-Dfiles=" + ",".join(f"{prefix}-{classifier}.jar" for classifier in classifiers),
               "-Dclassifiers=" + ",".join(classifiers), "-Dtypes=" + ",".join("jar" for _ in classifiers)]
    subprocess.run(command, check=True)


def read_remote(url, headers):
  for attempt in range(5):
    try:
      with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=120) as response:
        return response.read()
    except urllib.error.HTTPError as error:
      if error.code not in (404, 429, 502, 503) or attempt == 4:
        raise RuntimeError(f"Package verification failed: HTTP {error.code} at {url}") from None
      time.sleep(2 ** attempt)


def verify_remote(repository, value, headers, url=REGISTRY):
  for module in MODULES:
    artifact = f"bemo-{module}"
    directory = f"dev/elide/bemo/{artifact}/{value}"
    versions = {}
    snapshot_version = None
    if value.endswith("-SNAPSHOT"):
      metadata = ET.fromstring(read_remote(f"{url}/{directory}/maven-metadata.xml", headers))
      timestamp = metadata.findtext("versioning/snapshot/timestamp")
      number = metadata.findtext("versioning/snapshot/buildNumber")
      if timestamp and number:
        snapshot_version = f"{value.removesuffix('SNAPSHOT')}{timestamp}-{number}"
      for entry in metadata.findall("versioning/snapshotVersions/snapshotVersion"):
        versions[(entry.findtext("extension"), entry.findtext("classifier") or "")] = entry.findtext("value")
    for path in sorted((repository / directory).iterdir()):
      extension = path.suffix[1:]
      classifier = path.stem[len(f"{artifact}-{value}"):].removeprefix("-")
      # Registries may omit classifier entries; Maven also resolves snapshots
      # from the shared timestamp and build number. Still verify every byte.
      resolved = (versions.get((extension, classifier)) or snapshot_version) if value.endswith("-SNAPSHOT") else value
      if not resolved:
        raise RuntimeError(f"Published snapshot metadata is missing {path.name}")
      filename = f"{artifact}-{resolved}" + (f"-{classifier}" if classifier else "") + f".{extension}"
      content = read_remote(f"{url}/{directory}/{filename}", headers)
      if hashlib.sha256(content).digest() != hashlib.sha256(path.read_bytes()).digest():
        raise RuntimeError(f"Published Maven artifact differs from verified staging: {path.name}")
  print("Verified remote POMs, Java JARs, sources, Javadocs, and both native platforms")


def select():
  value = build.VERSION
  if value.endswith("-SNAPSHOT"):
    selected = True
  else:
    tag = release.release_tag()
    if tag is None:
      raise RuntimeError("Unsupported Maven release version")
    ref = release.gh("api", f"repos/{os.environ['GITHUB_REPOSITORY']}/git/ref/tags/{tag}")
    obj = json.loads(ref)["object"]
    while obj["type"] == "tag":
      obj = json.loads(release.gh("api", f"repos/{os.environ['GITHUB_REPOSITORY']}/git/tags/{obj['sha']}"))["object"]
    selected = obj["type"] == "commit" and obj["sha"] == os.environ["GITHUB_SHA"]
  with open(os.environ["GITHUB_OUTPUT"], "a") as output:
    output.write(f"publish={'true' if selected else 'false'}\n")


def publish(repository, value):
  if (os.environ.get("GITHUB_EVENT_NAME"), os.environ.get("GITHUB_REF"), os.environ.get("GITHUB_REPOSITORY")) != (
      "push", "refs/heads/main", "elide-dev/bemo"):
    raise RuntimeError("GitHub Packages publication requires a verified main push")
  if not os.environ.get("GH_TOKEN") or not os.environ.get("GITHUB_ACTOR"):
    raise RuntimeError("GitHub Packages publication requires workflow credentials")
  with tempfile.TemporaryDirectory(prefix="bemo-publish-") as directory:
    settings = Path(directory) / "settings.xml"
    settings.write_text('''<settings xmlns="http://maven.apache.org/SETTINGS/1.0.0">
  <servers><server><id>github</id>
    <username>${env.GITHUB_ACTOR}</username><password>${env.GH_TOKEN}</password>
  </server></servers>
</settings>
''')
    settings.chmod(0o600)
    deploy(repository, value, settings)
  credentials = f"{os.environ['GITHUB_ACTOR']}:{os.environ['GH_TOKEN']}".encode()
  verify_remote(repository, value, {"Authorization": "Basic " + base64.b64encode(credentials).decode()})


if __name__ == "__main__":
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument("task", choices=("select", "merge", "publish"))
  args = parser.parse_args()
  if args.task == "select":
    select()
    raise SystemExit(0)
  destination = merge(build.BUILD / "release-assets", build.BUILD / "github-packages", build.VERSION)
  if args.task == "publish":
    publish(destination, build.VERSION)
  print(f"GitHub Packages Maven repository: {destination}")
