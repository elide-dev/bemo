#!/usr/bin/env python3
"""Clean-cache GDB/debuginfod address qualification of manifest-bound shipped modules.

No uploads, credential minting, or producer-error waivers. Expectations are inputs
reviewed against source; this tool never learns expected answers from debug output.
"""
import argparse
import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import struct
import tempfile
import zlib

spec = importlib.util.spec_from_file_location("coverage_collect", Path(__file__).with_name("collect.py"))
collect = importlib.util.module_from_spec(spec)
spec.loader.exec_module(collect)

ingest_spec = importlib.util.spec_from_file_location("coverage_ingest", Path(__file__).with_name("ingest.py"))
ingest = importlib.util.module_from_spec(ingest_spec)
ingest_spec.loader.exec_module(ingest)
snapshot_spec = importlib.util.spec_from_file_location("coverage_snapshot", Path(__file__).with_name("source-snapshot.py"))
snapshot = importlib.util.module_from_spec(snapshot_spec)
snapshot_spec.loader.exec_module(snapshot)


def sha(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def gate_code():
    return {name: sha(Path(__file__).with_name(name)) for name in (
        "symbolicate.py", "make-symbolication-case.py", "symbolicate-ci.py", "run-symbolication-case.py",
        "source-snapshot.py", "regenerate-sqlite-source.py", "collect.py", "ingest.py", "ingest-ci.py")}


def sanitize(text, secret=""):
    if secret:
        text = text.replace(secret, "[REDACTED]")
    text = re.sub(r"https?://[^\s\"'<>]+", "[service URL]", text)
    return re.sub(r"dbg_[a-z]_[A-Za-z0-9_-]+", "[REDACTED]", text)


def validate_case(case):
    for key in ("repository", "sourceRevision", "manifestSha256", "moduleSha256", "debugSha256", "debugId"):
        if not case.get(key):
            raise ValueError("Missing binding: " + key)
    if not re.fullmatch(r"[0-9a-f]{40}", case["sourceRevision"]):
        raise ValueError("Expected exact source revision")
    for key in ("manifestSha256", "moduleSha256", "debugSha256"):
        if not re.fullmatch(r"[0-9a-f]{64}", case[key]):
            raise ValueError("Expected SHA-256 binding: "+key)
    for key in ("debugId", "foreignDebugId"):
        if not re.fullmatch(r"[0-9a-f]{8,128}", case.get(key, "")):
            raise ValueError("Expected canonical platform identity: "+key)
    if not case.get("frames"):
        raise ValueError("No independent frame expectations")
    for frame in case["frames"]:
        if not frame.get("sourceEvidence") or not frame.get("chain"):
            raise ValueError("Frame needs source evidence and a complete expected chain")
        if not frame["sourceEvidence"].get("dependency") and frame["sourceEvidence"].get("revision") != case["sourceRevision"]:
            raise ValueError("Frame source revision differs from shipped build")
        if int(frame["runtimePc"], 0) < int(frame["loadBias"], 0):
            raise ValueError("PC precedes module load bias")
        for entry in frame["chain"]:
            if not entry.get("function") or not entry.get("file") or not isinstance(entry.get("line"), int) or entry["line"] < 1:
                raise ValueError("Function/file/positive line expectations are mandatory")


def verify_source(case, source_root, dependency_source_root=None, source_snapshot_root=None):
    evidence_rows = [e for frame in case["frames"] for e in [frame["sourceEvidence"], *frame.get("chainSources", [])]]
    for evidence in evidence_rows:
        if evidence.get("snapshot"):
            if source_snapshot_root is None or evidence["file"] != "sqlite3.c":
                raise ValueError("Provide the reviewed regenerated SQLite compilation source")
            content = (source_snapshot_root/"sqlite3.c").read_bytes()
            proof_path = source_snapshot_root/"provenance.json"
            proof = json.loads(proof_path.read_text())
            config = subprocess.check_output(["git", "-C", str(source_root), "show", case["sourceRevision"]+":tools/toolchains/native.mts"])
            if not (sha(proof_path) == evidence["provenanceSha256"] and proof["runtimeRevision"] == case["sourceRevision"]
                    and hashlib.sha256(config).hexdigest() == proof["runtimeToolchainConfigSha256"]
                    and hashlib.sha256(content).hexdigest() == evidence["sha256"] == proof["generatedSourceSha256"]
                    and snapshot.source_md5(content) == evidence["compilerMD5"] == proof["generatedSourceMD5"]):
                raise ValueError("Regenerated source/pin provenance differs from the reviewed build case")
            continue
        folder = source_root
        if evidence.get("dependency"):
            link = subprocess.check_output(["git", "-C", str(source_root), "ls-tree", case["sourceRevision"],
                                            evidence["dependency"]], text=True).split()
            if len(link) < 3 or link[0] != "160000" or link[2] != evidence["revision"] or dependency_source_root is None:
                raise ValueError("Dependency source is not bound by the producer's exact gitlink")
            folder = dependency_source_root
        content = subprocess.check_output(["git", "-C", str(folder), "show",
                                           evidence["revision"] + ":" + evidence["file"]])
        if hashlib.sha256(content).hexdigest() != evidence["sha256"]:
            raise ValueError("Independent source snapshot hash differs")


def outcome(result):
    integration = (result.get("remoteAvailability") == "passed" and result.get("pairingIntegrity") == "passed"
                   and bool(result.get("controls")) and all(c["passed"] for c in result["controls"].values()))
    producer = result.get("producerQuality")
    quality = (result.get("sourceInline") == "passed" and isinstance(producer, dict)
               and producer.get("status") == "passed" and result.get("sourceWorktreeDirty") is False)
    return {"integration": "passed" if integration else "failed",
            "quality": "passed" if quality else "failed"}


def verify_module(case, module, manifest):
    validate_case(case)
    if sha(module) != case["moduleSha256"] or sha(manifest) != case["manifestSha256"]:
        raise ValueError("Shipped module/manifest hash differs from reviewed case")
    data = json.loads(Path(manifest).read_text())
    if data["build"]["repository"] != case["repository"] or data["build"]["commit"] != case["sourceRevision"]:
        raise ValueError("Manifest source identity differs")
    matches = [m for m in data["modules"] if m["path"] == case["modulePath"] and m["sha256"] == case["moduleSha256"]]
    if len(matches) != 1:
        raise ValueError("Expected one exact shipped manifest member")
    meta = collect.native(module)
    if not meta or meta["format"] not in ("elf", "macho") or meta["dwarf"]:
        raise ValueError("Address gates require a stripped shipped ELF or thin Mach-O module")
    if meta["identity"]["value"] != case["debugId"] or meta["architecture"] != case["architecture"]:
        raise ValueError("Byte-derived module identity/architecture differs")
    if not any(d.get("sha256") == case["debugSha256"] for d in matches[0].get("debug", {}).get("artifacts", [])):
        raise ValueError("Expected debug hash is absent from the manifest pairing")
    with Path(module).open("rb") as stream:
        header = stream.read(64)
        ranges = []
        if meta["format"] == "elf":
            endian = "<" if header[5] == 1 else ">"
            if header[4] != 2:
                raise ValueError("This address gate currently supports ELF64")
            offset = struct.unpack_from(endian+"Q", header, 32)[0]
            size, count = struct.unpack_from(endian+"HH", header, 54)
            for index in range(count):
                stream.seek(offset+size*index)
                kind, flags, _, address, _, _, length, _ = struct.unpack(endian+"IIQQQQQQ", stream.read(56))
                if kind == 1 and flags & 1:
                    ranges.append((address, address+length))
        else:
            if header[:4] not in (b"\xcf\xfa\xed\xfe", b"\xfe\xed\xfa\xcf"):
                raise ValueError("This address gate currently supports thin Mach-O64")
            endian = "<" if header[0] == 0xcf else ">"
            count = struct.unpack_from(endian+"I", header, 16)[0]
            offset = 32
            for _ in range(count):
                stream.seek(offset)
                kind, size = struct.unpack(endian+"II", stream.read(8))
                if kind == 0x19:
                    stream.seek(offset+24)
                    address, length = struct.unpack(endian+"QQ", stream.read(16))
                    stream.seek(offset+60)
                    protection = struct.unpack(endian+"I", stream.read(4))[0]
                    if protection & 4:
                        ranges.append((address, address+length))
                offset += size
    for frame in case["frames"]:
        pc = int(frame["runtimePc"], 0)-int(frame["loadBias"], 0)
        if not any(start <= pc < end for start, end in ranges):
            raise ValueError("Normalized PC is outside shipped executable load ranges")
    return meta


def compare_chain(actual, expected):
    if len(actual) != len(expected):
        return False
    return all(re.fullmatch(want["function"], got["FunctionName"]) is not None
               and file_matches(got["FileName"], want["file"])
               and got["Line"] == want["line"] for got, want in zip(actual, expected))


def file_matches(actual, expected):
    actual = actual.replace("\\", "/")
    return actual == expected or actual.endswith("/"+expected.lstrip("/"))


def producer_quality(manifest, module_hash, retention_path=None):
    data = json.loads(Path(manifest).read_text())
    records = [retention["record"] for module in data["modules"] if module["sha256"] == module_hash
               for retention in module.get("debug", {}).get("retention", [])
               if retention["association"] == "exact-distributed-bytes"
               and retention["record"]["binary"]["sha256"] == module_hash]
    if retention_path is not None:
        external = json.loads(Path(retention_path).read_text())
        if external["binary"]["sha256"] != module_hash:
            raise ValueError("External retention record differs from shipped module bytes")
        if records and not any(external == record for record in records):
            raise ValueError("External retention differs from the manifest's retained record")
        records = [external]
    verifications = [r["dwarf"]["verification"] for r in records]
    return {"status": "passed" if verifications and all(v["status"] == "passed" and v.get("exitCode") == 0 for v in verifications) else "failed",
            "verification": verifications, "reason": "Exact-byte manifest retention; missing or producer-errors cannot pass"}


def http_status(server, debug_id, secret, org=""):
    headers = []
    if secret:
        if secret.startswith("dbg_q_"):
            headers.append("Authorization: Basic " + base64.b64encode((secret + ":").encode()).decode())
        else:
            headers.append("Authorization: Bearer " + secret)
    if org:
        headers.append("X-Debuginfo-Tenant: " + org)
    if any(char in secret+org for char in "\r\n"):
        raise ValueError("Control credentials/tenant must be single-line values")
    # Use the ordinary supported curl client, as the positive CI GET does.
    # Python's default HTTP agent is rejected by staging's browser-integrity
    # filter before the Worker. No user-agent disguise or firewall change.
    config = "".join("header = "+json.dumps(header)+"\n" for header in headers)
    result = subprocess.run(["curl", "--disable", "--silent", "--show-error", "--max-time", "60",
                             "--connect-timeout", "30", "--max-filesize", "1", "--output", os.devnull,
                             "--config", "-", "--write-out", "%{http_code}",
                             server+"/buildid/"+debug_id+"/debuginfo"],
                            input=config, capture_output=True, text=True, timeout=70)
    # 63 is the deliberate one-byte body bound; headers still report the real
    # status. Redirects are not followed, and no secret appears in argv.
    if result.returncode not in (0, 63) or not re.fullmatch(r"[1-5][0-9]{2}", result.stdout):
        raise RuntimeError("HTTP control failed: "+sanitize(result.stderr, secret))
    return int(result.stdout)


def clean_env(root):
    # Deliberate allow-list: no inherited debug paths, credentials, compiler paths,
    # source paths, preload hooks, or system debuginfod URLs.
    env = {k: os.environ[k] for k in ("PATH", "SSL_CERT_FILE", "SSL_CERT_DIR") if k in os.environ}
    for name, folder in (("HOME", "home"), ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"),
                         ("DEBUGINFOD_CACHE_PATH", "debuginfod")):
        path = root/folder
        path.mkdir()
        env[name] = str(path)
    env.update({"LC_ALL": "C", "TELEMETRY": "off", "DEBUGINFO_DEV_POSTHOG_KEY": "", "DEBUGINFOD_URLS": "",
                "DEBUGINFOD_TIMEOUT": "120", "DEBUGINFOD_RETRY_LIMIT": "0"})
    return env


def command(argv, env, cwd, secret):
    result = subprocess.run(argv, env=env, cwd=cwd, capture_output=True, text=True, timeout=600)
    if result.returncode:
        raise RuntimeError("Client failed: " + sanitize(result.stderr + result.stdout, secret)[-2000:])
    return result.stdout


def live_call(case, shipped, env, root, secret):
    probe = case["liveCall"]
    name = probe["function"]
    if not re.fullmatch(r"[A-Za-z_][A-Za-z_0-9]*", name):
        raise ValueError("Live probe requires a reviewed no-argument uint32 C export")
    driver = root/"call.py"
    driver.write_text("import ctypes\nlib=ctypes.CDLL("+repr(str(shipped))+")\n"+
                      "fn=getattr(lib,"+repr(name)+")\nfn.restype=ctypes.c_uint32\n"+
                      "print('GATE_CALL_RESULT='+str(fn()),flush=True)\n")
    inspect = root/"live.py"
    inspect.write_text("import gdb,json\n"+
                       "pc=int(gdb.selected_frame().pc())\n"+
                       "entry=int(gdb.parse_and_eval('&"+name+"'))\n"+
                       "sal=gdb.find_pc_line(pc)\n"+
                       "print('GATE_LIVE:'+json.dumps({'runtimePc':hex(pc),'runtimeFunctionEntry':hex(entry),"+
                       "'function':gdb.selected_frame().name(),'file':sal.symtab.fullname() if sal.symtab else None,'line':sal.line}))\n")
    output = command(["gdb", "-q", "-batch", "-iex", "set auto-load safe-path /nonexistent",
                      "-ex", "set debuginfod enabled on", "-ex", "set debug-file-directory /nonexistent",
                      "-ex", "set directories /nonexistent", "-ex", "set disable-randomization off",
                      "-ex", "set breakpoint pending on", "-ex", "break "+name, "-ex", "run",
                      "-ex", "source "+str(inspect), "-ex", "continue",
                      "--args", shutil.which("python3"), str(driver)], env, root, secret)
    records = [json.loads(line.split("GATE_LIVE:", 1)[1]) for line in output.splitlines() if line.startswith("GATE_LIVE:")]
    if len(records) != 1:
        raise ValueError("Live debugger did not stop in the requested shipped function")
    row = records[0]
    bias = int(row["runtimeFunctionEntry"], 0)-int(probe["objectFunctionPc"], 0)
    normalized = int(row["runtimePc"], 0)-bias
    expected = probe["expected"]
    row.update({"loadBias": hex(bias), "objectPc": hex(normalized), "aslrEnabled": True,
                "passed": (bias != 0 and normalized == int(probe["objectFunctionPc"], 0)
                           and row["function"] == name and row["file"] is not None
                           and file_matches(row["file"], expected["file"]) and row["line"] == expected["line"]
                           and "GATE_CALL_RESULT="+str(probe["returnValue"]) in output),
                "transcript": sanitize(output, secret)})
    return row


def qualify(case, module, manifest, client, symbolizer, server, secret, mode="automatic-gdb", org="", retention_path=None):
    meta = verify_module(case, module, manifest)
    if retention_path is not None and not case.get("preservationRecordSha256"):
        raise ValueError("External preservation evidence must be bound by the reviewed case")
    if case.get("preservationRecordSha256"):
        if retention_path is None or sha(retention_path) != case["preservationRecordSha256"]:
            raise ValueError("Provide the exact bound external preservation record")
        retention = json.loads(Path(retention_path).read_text())
        if retention["debug"]["sha256"] != case["debugSha256"]:
            raise ValueError("External preservation record differs from expected remote debug bytes")
    finder = mode == "debuginfod-find"
    query_mode = mode == "automatic-gdb" or finder
    if not finder and meta["format"] != "elf":
        raise ValueError("Use the explicit debuginfod-find/LLVM path for cross-platform Mach-O address samples")
    result = {"repository": case["repository"], "sourceRevision": case["sourceRevision"],
              "modulePath": case["modulePath"], "moduleSha256": case["moduleSha256"],
              "manifestSha256": case["manifestSha256"], "debugId": case["debugId"],
              "architecture": case["architecture"], "debugSha256": case["debugSha256"],
              "remoteAvailability": "not-tested", "pairingIntegrity": "not-tested", "sourceInline": "not-tested",
              "sourceCorrectness": "not-tested", "inlineCorrectness": "not-tested",
              "inlineProbeCount": sum(len(frame["chain"]) > 1 for frame in case["frames"]),
              "variables": "not-tested", "execution": "offline-address-symbolization; no inferior executed",
              "qualificationScope": "reviewed PCs and retained producer verification; not whole-module source/inline/variable coverage",
              "producerQuality": producer_quality(manifest, case["moduleSha256"], retention_path),
              "sourceWorktreeDirty": json.loads(Path(manifest).read_text())["build"].get("trackedWorktreeDirty"), "frames": []}
    result["gateCodeSha256"] = gate_code()
    result["tools"] = {}
    for label, executable in (("debuginfod-find", "debuginfod-find") if finder else ("gdb", "gdb"),
                               ("symbolizer", symbolizer), ("client", client)):
        resolved = shutil.which(executable)
        if resolved is None:
            raise ValueError("Required client tool is unavailable: "+label)
        version = subprocess.check_output([resolved, "--version"], text=True)
        result["tools"][label] = {"sha256": sha(resolved), "version": version.strip()}
    with tempfile.TemporaryDirectory(prefix="symbolicate-clean-") as temporary:
        root = Path(temporary)
        env = clean_env(root)
        shipped = root/"shipped"
        shutil.copyfile(module, shipped)
        if query_mode:
            installed = root/"bin"
            installed.mkdir()
            (installed/"dbginfo").symlink_to(Path(shutil.which(client)).resolve())
            env["PATH"] = str(installed)+os.pathsep+env["PATH"]
            env["DEBUGINFO_DEV_QUERY_KEY"] = secret
            command([client, "--no-telemetry", "--url", server, "setup", "--shell", "bash"], env, root, secret)
            env.pop("DEBUGINFO_DEV_QUERY_KEY")
            # Start the fresh interactive shell setup configured. Capture its
            # environment privately; never print credential-bearing URLs.
            capture = root/"shell-env.py"
            capture.write_text("import os,json\nprint(json.dumps({'urls':os.environ.get('DEBUGINFOD_URLS','')}))\n")
            import shlex
            rendered = command(["bash", "--noprofile", "--rcfile", str(Path(env["HOME"])/".bashrc"), "-ic",
                                "python3 "+shlex.quote(str(capture))], env, root, secret)
            urls = json.loads(rendered)["urls"].split()
            if not urls:
                raise ValueError("The configured fresh shell did not enable private debuginfod access")
            # Exclude distro fallbacks from this qualification, preserving the
            # ordinary CLI-generated private server entry byte for byte.
            env["DEBUGINFOD_URLS"] = urls[0]
            result["setupIntegration"] = "fresh interactive bash reading the setup-written rc file"
            safe_server = server.replace("https://api.", "https://")
        else:
            safe_server = server
        controls = {}
        for label, identity, credential, expected in (
            ("noCredential", case["debugId"], "", 401),
            ("invalidCredential", case["debugId"], "dbg_q_" + "0"*48, 401),
            ("wrongPrivateProject", case["foreignDebugId"], secret, 403),
            ("missingIdentity", "f"*40, secret, 404),
        ):
            received = http_status(safe_server, identity, credential, org)
            controls[label] = {"status": received, "expected": expected, "passed": received == expected}
        result["controls"] = controls
        # The debugger performs the first and only positive download. There are
        # no local debug peers, build-id directories, DWO files or source trees.
        script = root/"lookup.py"
        script.write_text("import gdb,json\n" +
                          "pcs=" + repr([int(f["runtimePc"], 0)-int(f["loadBias"], 0) for f in case["frames"]]) + "\n" +
                          "rows=[]\nfor pc in pcs:\n" +
                          " s=gdb.find_pc_line(pc)\n" +
                          " rows.append({'pc':hex(pc),'file':s.symtab.fullname() if s.symtab else None,'line':s.line})\n" +
                          "print('GATE_JSON:'+json.dumps({'lines':rows,'objfiles':[o.filename for o in gdb.objfiles()]}))\n")
        denied_root = root/"denied"
        denied_root.mkdir()
        denied_env = clean_env(denied_root)
        denied_env["DEBUGINFOD_URLS"] = safe_server
        # A headerless client uses the global PUBLIC index, whose private-only
        # identities are absent. Scoped HTTP controls above exercise org auth.
        result["noCredentialClientRoute"] = "global-public-index; private identity absent"
        if finder:
            denied = subprocess.run(["debuginfod-find", "debuginfo", case["debugId"]], env=denied_env, cwd=denied_root,
                                    capture_output=True, text=True, timeout=180)
            denied_ok = denied.returncode != 0 and not any(p.stat().st_size for p in (denied_root/"debuginfod").rglob("debuginfo") if p.is_file())
            controls["noCredentialClient"] = {"passed": denied_ok}
        else:
            denied = command(["gdb", "-nx", "-q", "-batch", "-ex", "set debuginfod enabled on",
                          "-ex", "set debug-file-directory /nonexistent", "-ex", "set directories /nonexistent",
                          "-ex", "file "+str(shipped), "-ex", "source "+str(script)], denied_env, denied_root, secret)
            denied_rows = [json.loads(line.split("GATE_JSON:", 1)[1]) for line in denied.splitlines() if line.startswith("GATE_JSON:")]
            denied_ok = (len(denied_rows) == 1 and all(row["file"] is None for row in denied_rows[0]["lines"])
                     and not any(p.stat().st_size for p in (denied_root/"debuginfod").rglob("debuginfo") if p.is_file()))
            controls["noCredentialDebugger"] = {"passed": denied_ok, "sourceLinesUnavailable": denied_ok}
        debugger_args = ["gdb", "-q", "-batch", "-iex", "set auto-load safe-path /nonexistent",
                         "-ex", "set debug-file-directory /nonexistent", "-ex", "set directories /nonexistent"]
        if finder:
            output = command(["debuginfod-find", "debuginfo", case["debugId"]], env, root, secret)
            lookup = {"lines": [None]*len(case["frames"])}
        elif mode == "automatic-gdb":
            output = command([*debugger_args, "-ex", "file " + str(shipped), "-ex", "source " + str(script)], env, root, secret)
        else:
            env["DEBUGINFO_DEV_APIKEY"] = secret
            env["DEBUGINFO_DEV_ORG"] = org
            peer = root/"retrieved.debug"
            if mode == "manual-http":
                if secret != os.environ.get("DEBUGINFO_DEV_APIKEY"):
                    raise ValueError("HTTP CI mode requires the existing action credential")
                ingest.download(server, org, case["debugId"], peer)
            else:
                command([client, "--no-telemetry", "--url", server, "get", case["debugId"], "-o", str(peer)], env, root, secret)
            env.pop("DEBUGINFO_DEV_APIKEY")
            output = command([*debugger_args, "-nx", "-ex", "set debuginfod enabled off", "-ex", "file "+str(shipped),
                              "-ex", "symbol-file "+str(peer), "-ex", "source "+str(script)], env, root, secret)
        if not finder:
            records = [json.loads(line.split("GATE_JSON:", 1)[1]) for line in output.splitlines() if line.startswith("GATE_JSON:")]
            if len(records) != 1:
                raise ValueError("No independent GDB lookup result")
            lookup = records[0]
        if len(lookup["lines"]) != len(case["frames"]):
            raise ValueError("Debugger did not return a result for every reviewed address")
        peers = ([p for p in (root/"debuginfod").rglob("debuginfo") if p.is_file() and p.stat().st_size]
                 if query_mode else [peer])
        if len(peers) != 1 or (str(peers[0]) != output.strip() if finder else str(peers[0]) not in lookup["objfiles"]):
            raise ValueError("Client did not use exactly one freshly downloaded peer")
        peer = peers[0]
        result["remoteAvailability"] = "passed"
        result["retrieval"] = {"method": mode, "cacheInitiallyEmpty": True,
                               "loadedFromCache": query_mode, "sha256": sha(peer), "bytes": peer.stat().st_size,
                               "clientOutput": sanitize(output, secret), "nativeDebuggerAutomatic": mode == "automatic-gdb"}
        peer_meta = collect.native(peer)
        crc = 0
        with peer.open("rb") as stream:
            while chunk := stream.read(1024*1024):
                crc = zlib.crc32(chunk, crc)
        result["retrieval"]["crc32"] = crc
        paired = (sha(peer) == case["debugSha256"] and peer_meta["identity"] == meta["identity"]
                  and peer_meta["architecture"] == meta["architecture"]
                  and (not meta["debuglink"] or crc == meta["debuglink"]["crc32"]))
        result["pairingIntegrity"] = "passed" if paired else "failed"
        if not paired:
            return result
        result["sourceSnapshots"] = []
        for frame in case["frames"]:
            evidence = frame["sourceEvidence"]
            if not evidence.get("snapshot"):
                continue
            dwarfdump = str(Path(shutil.which(symbolizer)).with_name("llvm-dwarfdump"))
            result["tools"]["dwarfdump"] = {"sha256": sha(dwarfdump),
                                                "version": subprocess.check_output([dwarfdump, "--version"], text=True).strip()}
            diagnostics = {}
            try:
                observed = snapshot.compiler_md5(peer, evidence["compiledFunction"], evidence["file"], dwarfdump, diagnostics)
            except Exception as error:
                result["sourceSnapshots"].append({"file": evidence["file"], "passed": False,
                                                 "error": sanitize(str(error), secret), "diagnostics": diagnostics})
                result["sourceInline"] = "failed"
                result["sourceCorrectness"] = "unverified"
                return result
            result["sourceSnapshots"].append({"file": evidence["file"], "sourceSha256": evidence["sha256"],
                                             "compiledSourceMD5": observed, "expectedMD5": evidence["compilerMD5"],
                                             "passed": observed == evidence["compilerMD5"], "provenanceSha256": evidence["provenanceSha256"],
                                             "diagnostics": diagnostics})
        if any(not row["passed"] for row in result["sourceSnapshots"]):
            result["sourceInline"] = "failed"
            result["sourceCorrectness"] = "unverified"
            return result
        for frame, gdb_line in zip(case["frames"], lookup["lines"]):
            pc = int(frame["runtimePc"], 0)-int(frame["loadBias"], 0)
            expected = frame["chain"]
            try:
                raw = command([symbolizer, "--output-style=JSON", "--inlines", "--obj="+str(peer), hex(pc)], env, root, secret)
                decoded = json.loads(raw)
                if not isinstance(decoded, list) or len(decoded) != 1:
                    raise ValueError("Expected one symbolizer result for one address")
                symbols = decoded[0]["Symbol"]
            except Exception as error:
                result["frames"].append({"objectPc": hex(pc), "expected": expected, "passed": False,
                                         "sourcePassed": False, "inlinePassed": False if len(expected) > 1 else None,
                                         "sourceEvidence": frame["sourceEvidence"], "error": sanitize(str(error), secret)})
                continue
            gdb_matches = finder or (gdb_line["file"] is not None and file_matches(gdb_line["file"], expected[0]["file"])
                                    and gdb_line["line"] == expected[0]["line"])
            result["frames"].append({"runtimePc": frame["runtimePc"], "loadBias": frame["loadBias"], "objectPc": hex(pc),
                                     "sourceEvidence": frame["sourceEvidence"], "expected": expected,
                                     "actual": symbols, "gdb": gdb_line,
                                     "sourcePassed": bool(symbols) and compare_chain(symbols[:1], expected[:1]) and gdb_matches,
                                     "inlinePassed": compare_chain(symbols, expected) if len(expected) > 1 else None,
                                     "passed": compare_chain(symbols, expected) and gdb_matches})
        result["sourceInline"] = "passed" if all(f["passed"] for f in result["frames"]) else "failed"
        result["sourceCorrectness"] = "passed" if all(f["sourcePassed"] for f in result["frames"]) else "failed"
        inline_frames = [frame for frame in result["frames"] if frame["inlinePassed"] is not None]
        result["inlineCorrectness"] = ("passed" if all(frame["inlinePassed"] for frame in inline_frames) else "failed") if inline_frames else "not-tested"
        if case.get("liveCall"):
            if mode != "automatic-gdb":
                result["liveExecution"] = {"passed": False, "reason": "Live probe requires the automatic developer setup mode"}
            else:
                result["liveExecution"] = live_call(case, shipped, env, root, secret)
                result["execution"] = "real shipped library loaded by ctypes; GDB breakpoint; observed ASLR bias"
            if not result["liveExecution"]["passed"]:
                result["sourceInline"] = "failed"
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("case", "module", "manifest", "source-root", "out"):
        parser.add_argument("--"+name, type=Path, required=True)
    parser.add_argument("--query-key-file", type=Path)
    parser.add_argument("--dependency-source-root", type=Path)
    parser.add_argument("--source-snapshot-root", type=Path)
    parser.add_argument("--preservation-record", type=Path)
    parser.add_argument("--mode", choices=("automatic-gdb", "debuginfod-find", "manual-cli", "manual-http"), default="automatic-gdb")
    parser.add_argument("--org", default=os.environ.get("DEBUGINFO_DEV_ORG", ""))
    parser.add_argument("--execution-nonce")
    parser.add_argument("--client", default="dbginfo")
    parser.add_argument("--symbolizer", default="llvm-symbolizer")
    parser.add_argument("--server", default="https://api.staging.debuginfo.dev")
    args = parser.parse_args()
    case = json.loads(args.case.read_text())
    secret = args.query_key_file.read_text().strip() if args.query_key_file else os.environ.get("DEBUGINFO_DEV_APIKEY", "")
    try:
        if not args.org:
            raise ValueError("Supply --org for the scoped private-project authentication controls")
        if args.mode in ("automatic-gdb", "debuginfod-find") and not secret.startswith("dbg_q_"):
            raise ValueError("Use authorized project-scoped QUERY access")
        if args.mode not in ("automatic-gdb", "debuginfod-find") and (not secret or not args.org):
            raise ValueError("Use the existing project's CI credential and org")
        destination = None
        if args.mode == "manual-http":
            identity = ingest.cli("--url", args.server, "--org", args.org, "whoami")[0]
            credential = identity.get("credential", {})
            attestation = credential.get("attestation") or {}
            if (credential.get("kind") != "signed" or credential.get("scope") != "WRITE"
                    or attestation.get("repo") != case["repository"] or attestation.get("ref") != "refs/heads/main"
                    or attestation.get("sha") != os.environ.get("GITHUB_SHA")):
                raise ValueError("CI address gates require the existing action's current-main project credential")
            destination = ingest.private_project(args.server, args.org, os.environ["DEBUGINFO_DEV_ORG_ID"],
                                                 os.environ["DEBUGINFO_DEV_PROJECT"], os.environ["DEBUGINFO_DEV_PROJECT_ID"])
            destination["credentialDisplay"] = credential
        verify_source(case, args.source_root, args.dependency_source_root, args.source_snapshot_root)
        result = qualify(case, args.module, args.manifest, args.client, args.symbolizer, args.server, secret, args.mode, args.org, args.preservation_record)
        result["destination"] = destination
    except Exception as error:
        result = {key: case.get(key) for key in ("repository", "sourceRevision", "modulePath", "moduleSha256", "manifestSha256", "debugSha256", "debugId", "architecture")}
        result.update({"status": "failed", "error": sanitize(str(error), secret), "remoteAvailability": "not-tested",
                       "pairingIntegrity": "not-tested", "sourceInline": "not-tested", "sourceCorrectness": "not-tested",
                       "inlineCorrectness": "not-tested", "variables": "not-tested"})
    result["outcome"] = outcome(result)
    result["executionNonce"] = args.execution_nonce
    result["caseSha256"] = sha(args.case)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2)+"\n")
    passed = all(value == "passed" for value in result["outcome"].values())
    raise SystemExit(0 if passed else 1)


if __name__ == "__main__":
    main()
