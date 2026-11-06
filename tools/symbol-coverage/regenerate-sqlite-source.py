#!/usr/bin/env python3
"""Regenerate owned SQLite JNI compilation source from a Runtime revision's pins.

Only pinned Elide SQLite/JNI sources are used. No symbol uploads, account access
changes, or producer edits occur. The downloaded peer's DWARF source MD5 must
subsequently agree; a declared dependency version alone cannot certify inclusion.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess


def gh_json(endpoint):
    return json.loads(subprocess.check_output([os.environ.get("GH_BIN", "gh"), "api", endpoint], text=True))


def blob(repository, revision, path):
    value = gh_json(f"repos/{repository}/contents/{path}?ref={revision}")
    if value.get("encoding") != "base64":
        raise ValueError("Expected a pinned GitHub source blob")
    return base64.b64decode(value["content"])


def sha(content):
    return hashlib.sha256(content).hexdigest()


def regenerate(source_root, revision, output, toolchain_manifest=None):
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("Expected exact Runtime source revision")
    runtime_source = subprocess.check_output(["git", "-C", str(source_root), "show", revision+":tools/toolchains/native.mts"])
    text = runtime_source.decode()
    versions = re.findall(r'NATIVE_TOOLCHAIN_VERSION = "([A-Za-z0-9._-]+)"', text)
    if len(versions) != 1:
        raise ValueError("Expected one native-toolchain source binding")
    version = versions[0]
    configured = text.split('"linux-amd64": toolchainArchive({', 1)[1].split("}),", 1)[0]
    checksums = re.findall(r'"([0-9a-f]{64})"', configured)
    if len(checksums) != 1:
        raise ValueError("Expected exact configured Linux toolchain archive SHA")
    tag_object = None
    if toolchain_manifest is not None:
        manifest = json.loads(Path(toolchain_manifest).read_text())
        build = manifest["build"]
        expected_name = "elide-toolchain-"+version+"-linux-amd64.tar.xz"
        if (build["repository"] != "elide-dev/toolchain" or build["release"] != version
                or not re.fullmatch(r"[0-9a-f]{40}", build["commit"])
                or not any(c["path"].endswith(expected_name) and c["sha256"] == checksums[0] for c in manifest["containers"])):
            raise ValueError("Upstream toolchain manifest does not bind the configured exact native archive")
        toolchain = build["commit"]
    else:
        tag = gh_json("repos/elide-dev/toolchain/git/ref/tags/v"+version)["object"]
        tag_object = tag["sha"]
        for _ in range(8):
            if tag["type"] == "commit":
                break
            if tag["type"] != "tag":
                raise ValueError("Unexpected toolchain tag object")
            tag = gh_json("repos/elide-dev/toolchain/git/tags/"+tag["sha"])["object"]
        if tag["type"] != "commit":
            raise ValueError("Toolchain tag did not resolve to a commit")
        toolchain = tag["sha"]
    versions_source = blob("elide-dev/toolchain", toolchain, "versions.env")
    component = blob("elide-dev/toolchain", toolchain, "scripts/components/sqlite3elide.sh")
    pins = dict(re.findall(r"^([A-Z_0-9]+)=([^\s#]+)$", versions_source.decode(), re.M))
    for key in ("SQLITE_REV", "SQLITE_JNI_REV"):
        if not re.fullmatch(r"[0-9a-f]{40}", pins[key]):
            raise ValueError("Expected exact SQLite/JNI source revisions")
    output.mkdir(parents=True, exist_ok=False)
    source = output/"sqlite-source"
    subprocess.run(["git", "init", "-q", str(source)], check=True)
    subprocess.run(["git", "-C", str(source), "remote", "add", "origin", "https://github.com/elide-tools/sqlite.git"], check=True)
    subprocess.run(["git", "-C", str(source), "fetch", "--filter=blob:none", "--depth=1", "origin", pins["SQLITE_REV"]], check=True)
    subprocess.run(["git", "-C", str(source), "-c", "core.autocrlf=false", "checkout", "--detach", "FETCH_HEAD"], check=True)
    owned_source = None
    for filename, pin in (("sqlite3jni.c", "SQLITE_JNI_C_SHA256"), ("sqlite3jni.h", "SQLITE_JNI_H_SHA256")):
        data = blob("elide-tools/sqlite-jni", pins["SQLITE_JNI_REV"], "jni/"+filename)
        if sha(data) != pins[pin]:
            raise ValueError("Owned JNI input differs from pinned toolchain source")
        (source/filename).write_bytes(data)
        if filename.endswith(".c"):
            owned_source = data
    # These are source-generation flags from the pinned component, not a new
    # compiler/optimizer policy. Refuse a changed recipe until reviewed.
    flags = ["--enable-all", "--disable-debug", "--enable-static", "--disable-shared", "--column-metadata", "--geopoly",
             "--memsys5", "--scanstatus", "--update-limit", "--with-tempstore=yes", "--amalgamation-extra-src=sqlite3jni.c",
             "--disable-math", "--disable-tcl"]
    if any(flag not in component.decode() for flag in flags):
        raise ValueError("Pinned SQLite source-generation recipe changed; review required")
    env = {key: os.environ[key] for key in ("PATH", "HOME") if key in os.environ}
    env["LC_ALL"] = "C"
    for name, argv in (("configure", [str(source/"configure"), *flags]), ("generate", ["make", "-j2", "sqlite3.c"])):
        with (output/(name+".log")).open("w") as log:
            subprocess.run(argv, cwd=source, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=300, check=True)
    generated = (source/"sqlite3.c").read_bytes()
    (output/"sqlite3.c").write_bytes(generated)
    proof = {"runtimeRevision": revision, "runtimeToolchainConfigSha256": sha(runtime_source),
             "declaredNativeToolchain": version, "declaredLinuxToolchainArchiveSha256": checksums[0],
             "toolchainTagObject": tag_object, "toolchainSource": toolchain,
             "toolchainManifestSha256": sha(Path(toolchain_manifest).read_bytes()) if toolchain_manifest else None,
             "versionsSha256": sha(versions_source), "componentScriptSha256": sha(component),
             "sqliteRevision": pins["SQLITE_REV"], "jniRevision": pins["SQLITE_JNI_REV"],
             "jniCSourceSha256": sha(owned_source), "jniHeaderSha256": pins["SQLITE_JNI_H_SHA256"],
             "generatedSourceSha256": sha(generated), "generatedSourceMD5": hashlib.md5(generated, usedforsecurity=False).hexdigest(),
             "inclusion": "requires matching compiler source MD5 in the exact downloaded debug object"}
    (output/"provenance.json").write_text(json.dumps(proof, indent=2)+"\n")
    return proof


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--toolchain-manifest", type=Path)
    args = parser.parse_args()
    regenerate(args.source_root, args.revision, args.out, args.toolchain_manifest)


if __name__ == "__main__":
    main()
