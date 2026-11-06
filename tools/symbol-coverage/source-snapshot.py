"""Bind independently regenerated compilation sources to DWARF v5 file digests.

Reading a line-table header is metadata pairing, not a DWARF verifier pass.
Address/function/line expectations continue to come from reviewed source anchors.
"""
import hashlib
import base64
import gzip
import re
import subprocess
import tempfile
import threading


def compiler_md5(peer, function, filename, dwarfdump, diagnostics=None):
    result = subprocess.run([dwarfdump, "--name="+function, "--show-parents", str(peer)],
                            capture_output=True, text=True, timeout=300)
    if result.returncode:
        raise ValueError("Unable to read the compiled source's owning CU")
    offsets = set(re.findall(r"DW_AT_stmt_list\s*\((0x[0-9a-fA-F]+)\)", result.stdout))
    declared = re.findall(r'DW_AT_decl_file\s*\("([^"]+)"\)', result.stdout)
    if len(offsets) != 1 or not any(p.replace("\\", "/").endswith("/"+filename) or p == filename for p in declared):
        raise ValueError("Expected one owning CU and the reviewed compilation source")
    # Corrupt preceding line tables can produce substantial diagnostics before
    # the requested header. Drain to a file, not an unread stderr pipe; retain
    # those diagnostics without turning this metadata query into a verifier.
    with tempfile.TemporaryFile() as errors:
        process = subprocess.Popen([dwarfdump, "--debug-line="+offsets.pop(), str(peer)],
                                   stdout=subprocess.PIPE, stderr=errors, text=True)
        timer = threading.Timer(300, process.kill)
        timer.start()
        header, length = [], 0
        try:
            for line in process.stdout:
                if line.lstrip().startswith("Address"):
                    break
                header.append(line)
                length += len(line)
                if length > 2*1024*1024:
                    raise ValueError("Line-table header exceeds the supported bound")
        finally:
            process.terminate()
            process.stdout.close()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            timer.cancel()
        errors.seek(0)
        raw = result.stderr.encode()+errors.read()
    if diagnostics is not None:
        diagnostics.update({"sha256": hashlib.sha256(raw).hexdigest(), "bytes": len(raw),
                            "gzipBase64": base64.b64encode(gzip.compress(raw)).decode(),
                            "status": "diagnostics-present" if raw else "no-diagnostics",
                            "meaning": "metadata lookup only; does not waive retained producer verification failures"})
    values = []
    for block in re.split(r"file_names\[\s*\d+\]:", "".join(header))[1:]:
        name = re.search(r'name:\s*"([^"]+)"', block)
        checksum = re.search(r"md5_checksum:\s*([0-9a-fA-F]{32})", block)
        if name and name.group(1) == filename and checksum:
            values.append(checksum.group(1).lower())
    if len(set(values)) != 1:
        raise ValueError("Expected the compiler's exact source MD5; missing digests cannot qualify a snapshot")
    return values[0]


def source_md5(content):
    # This is DWARF's file-format checksum, not an authentication primitive.
    return hashlib.md5(content, usedforsecurity=False).hexdigest()
