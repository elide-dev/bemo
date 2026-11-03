#!/usr/bin/env python3
"""Symbol coverage v1: inspect final distributions, without uploading or executing them.

Canonical source: elide-dev/debuginfo.dev/tools/symbol-coverage. Python 3.11+,
stdlib only so the same collector can be vendored into existing packaging tools.
"""

import argparse
import hashlib
import json
import mmap
import os
import re
import shutil
import struct
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile
import zlib
from pathlib import Path, PurePosixPath

VERSION = 1
MAX_DEPTH = 8
MAX_EXPANDED = 128 * 1024**3


def digest(path):
    with open(path, "rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def artifact(path, name):
    checksum, crc = hashlib.sha256(), 0
    with open(path, "rb") as stream:
        while chunk := stream.read(1024 * 1024):
            checksum.update(chunk)
            crc = zlib.crc32(chunk, crc)
    return {
        "path": name,
        "sha256": checksum.hexdigest(),
        "size": Path(path).stat().st_size,
        "crc32": crc,
    }


def native(path):
    """Return byte-derived metadata. No filename- or host-derived identities."""
    if not Path(path).stat().st_size:
        return None
    with (
        open(path, "rb") as source,
        mmap.mmap(source.fileno(), 0, access=mmap.ACCESS_READ) as b,
    ):
        magic = b[:4]
        if magic == b"\x7fELF":
            endian = {1: "<", 2: ">"}[b[5]]
            wide = b[4] == 2
            kind, machine = struct.unpack_from(endian + "HH", b, 16)
            off = struct.unpack_from(
                endian + ("Q" if wide else "I"), b, 40 if wide else 32
            )[0]
            entsize, count, strings = struct.unpack_from(
                endian + "HHH", b, 58 if wide else 46
            )
            fmt = endian + ("IIQQQQIIQQ" if wide else "IIIIIIIIII")
            sections = [
                struct.unpack_from(fmt, b, off + i * entsize) for i in range(count)
            ]
            if not count or strings >= count:
                raise ValueError("ELF extended/absent section table is not supported")
            names = sections[strings]
            names = b[names[4] : names[4] + names[5]]
            ids, dwarf, debuglink, text_nobits = set(), False, None, False
            for section in sections:
                name = names[section[0] :].split(b"\0", 1)[0]
                data_off, size = section[4:6]
                if section[1] != 8 and data_off + size > len(b):
                    raise ValueError("ELF section extends beyond file")
                if name == b".text" and section[1] == 8:
                    text_nobits = True
                if (
                    name in (b".debug_info", b".zdebug_info")
                    and size
                    and section[1] != 8
                ):
                    dwarf = True
                if name == b".gnu_debuglink":
                    data = b[data_off : data_off + size]
                    end = data.index(0)
                    crc_off = (end + 4) & ~3
                    debuglink = {
                        "name": data[:end].decode("utf-8", "replace"),
                        "crc32": struct.unpack_from(endian + "I", data, crc_off)[0],
                    }
                if section[1] == 7:  # SHT_NOTE; debug companions may have no PT_NOTE.
                    pos, end = data_off, data_off + size
                    while pos + 12 <= end:
                        ns, ds, nt = struct.unpack_from(endian + "III", b, pos)
                        pos += 12
                        owner = b[pos : pos + ns].rstrip(b"\0")
                        pos += (ns + 3) & ~3
                        desc = b[pos : pos + ds]
                        pos += (ds + 3) & ~3
                        if owner == b"GNU" and nt == 3 and desc:
                            ids.add(desc.hex())
            if len(ids) > 1:
                raise ValueError("conflicting GNU build IDs")
            return {
                "format": "elf",
                "kind": "input"
                if kind == 1
                else "debug"
                if text_nobits and dwarf
                else "module",
                "architecture": {
                    62: "x86_64",
                    183: "aarch64",
                    3: "x86",
                    243: "riscv",
                }.get(machine, str(machine)),
                "identity": {"kind": "gnu-build-id", "value": next(iter(ids))}
                if ids
                else None,
                "dwarf": dwarf,
                "debuglink": debuglink,
            }
        if magic in (
            b"\xcf\xfa\xed\xfe",
            b"\xce\xfa\xed\xfe",
            b"\xfe\xed\xfa\xcf",
            b"\xfe\xed\xfa\xce",
        ):
            endian = "<" if magic[0] in (0xCF, 0xCE) else ">"
            wide = magic in (b"\xcf\xfa\xed\xfe", b"\xfe\xed\xfa\xcf")
            cpu, _, kind, count = struct.unpack_from(endian + "IIII", b, 4)
            pos, identity, dwarf = (32 if wide else 28), None, False
            for _ in range(count):
                cmd, size = struct.unpack_from(endian + "II", b, pos)
                if size < 8 or pos + size > len(b):
                    raise ValueError("invalid Mach-O load command")
                if cmd == 0x1B:
                    identity = {
                        "kind": "macho-uuid",
                        "value": b[pos + 8 : pos + 24].hex(),
                    }
                if cmd in (1, 0x19):
                    nsects = struct.unpack_from(
                        endian + "I", b, pos + (64 if cmd == 0x19 else 48)
                    )[0]
                    for i in range(nsects):
                        section = (
                            pos
                            + (72 if cmd == 0x19 else 56)
                            + i * (80 if cmd == 0x19 else 68)
                        )
                        section_size = struct.unpack_from(
                            endian + ("Q" if cmd == 0x19 else "I"),
                            b,
                            section + (40 if cmd == 0x19 else 36),
                        )[0]
                        dwarf |= (
                            b[section : section + 16].rstrip(b"\0") == b"__debug_info"
                            and section_size > 0
                        )
                pos += size
            return {
                "format": "macho",
                "kind": "input" if kind == 1 else "debug" if kind == 10 else "module",
                "architecture": {0x1000007: "x86_64", 0x100000C: "aarch64"}.get(
                    cpu, str(cpu)
                ),
                "identity": identity,
                "dwarf": dwarf,
                "debuglink": None,
            }
        if magic in (b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca", b"\xca\xfe\xba\xbf"):
            # Java classfiles share CAFEBABE. Fat Mach-O has a small nfat_arch.
            if magic == b"\xca\xfe\xba\xbe" and struct.unpack_from(">I", b, 4)[0] > 64:
                return None
            return {
                "format": "macho-fat",
                "kind": "module",
                "architecture": "multiple",
                "identity": None,
                "dwarf": False,
                "debuglink": None,
            }
        if b[:2] == b"MZ":
            pe = struct.unpack_from("<I", b, 0x3C)[0]
            if b[pe : pe + 4] != b"PE\0\0":
                raise ValueError("invalid PE signature")
            cpu, count = struct.unpack_from("<HH", b, pe + 4)
            opt_size = struct.unpack_from("<H", b, pe + 20)[0]
            opt = pe + 24
            directories = opt + (
                112 if struct.unpack_from("<H", b, opt)[0] == 0x20B else 96
            )
            rva, size = struct.unpack_from("<II", b, directories + 6 * 8)
            identity = None
            for i in range(count):
                section = opt + opt_size + i * 40
                vsize, va, rawsize, raw = struct.unpack_from("<IIII", b, section + 8)
                if rva and va <= rva < va + max(vsize, rawsize):
                    debug = raw + rva - va
                    for entry in range(debug, debug + size, 28):
                        dtype, length, _, pointer = struct.unpack_from(
                            "<IIII", b, entry + 12
                        )
                        if (
                            dtype == 2
                            and length >= 24
                            and b[pointer : pointer + 4] == b"RSDS"
                        ):
                            import uuid

                            guid = uuid.UUID(bytes_le=b[pointer + 4 : pointer + 20]).hex
                            age = struct.unpack_from("<I", b, pointer + 20)[0]
                            identity = {
                                "kind": "pdb-guid-age",
                                "value": f"{guid}-{age}",
                            }
            return {
                "format": "pe",
                "kind": "module",
                "architecture": {0x8664: "x86_64", 0xAA64: "aarch64"}.get(
                    cpu, str(cpu)
                ),
                "identity": identity,
                "dwarf": False,
                "debuglink": None,
            }
        if magic in (b"BC\xc0\xde", b"\xde\xc0\x17\x0b"):
            return {
                "format": "llvm-bitcode",
                "kind": "input",
                "architecture": "unverified",
                "identity": None,
                "dwarf": False,
                "debuglink": None,
            }
        if len(b) >= 20 and b[:2] in (b"\x64\x86", b"\x64\xaa", b"\x4c\x01"):
            return {
                "format": "coff-object",
                "kind": "input",
                "architecture": "byte-derived COFF",
                "identity": None,
                "dwarf": False,
                "debuglink": None,
            }
    return None


class Inventory:
    def __init__(self):
        self.files, self.containers, self.gaps, self.aliases = [], [], [], []
        self.expanded = 0

    def gap(self, path, reason):
        self.gaps.append({"path": path, "status": "unverified", "reason": reason})

    def member(self, source, size, name, depth):
        self.expanded += size
        if self.expanded > MAX_EXPANDED:
            raise ValueError("archive expansion limit exceeded")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "member"
            with path.open("wb") as temp:
                remaining = size
                while remaining:
                    chunk = source.read(min(1024 * 1024, remaining))
                    if not chunk:
                        raise ValueError("truncated archive member")
                    temp.write(chunk)
                    remaining -= len(chunk)
            self.scan(path, name, depth)

    def scan(self, path, name, depth=0):
        if depth > MAX_DEPTH:
            raise ValueError("archive nesting limit exceeded")
        with open(path, "rb") as source:
            head = source.read(8)
        lower = name.lower()
        try:
            if lower.endswith(".oci.tar"):
                self.oci(path, name, depth)
                return
            if head.startswith((b"PK\x03\x04", b"PK\x05\x06", b"JM\x01\x00PK")):
                self.containers.append({**artifact(path, name), "format": "zip"})
                with zipfile.ZipFile(path) as archive:
                    seen = set()
                    for item in archive.infolist():
                        if item.is_dir():
                            continue
                        member = self.name(item.filename, seen)
                        with archive.open(item) as stream:
                            if (item.external_attr >> 16) & 0o170000 == 0o120000:
                                self.aliases.append(
                                    {
                                        "path": name + "!/" + member,
                                        "target": stream.read(4096).decode(
                                            "utf-8", "replace"
                                        ),
                                    }
                                )
                                self.gap(
                                    name + "!/" + member,
                                    "archive alias target not resolved",
                                )
                            else:
                                self.member(
                                    stream,
                                    item.file_size,
                                    name + "!/" + member,
                                    depth + 1,
                                )
                return
            if head == b"!<arch>\n":
                self.containers.append({**artifact(path, name), "format": "ar"})
                # Debian packages use ar as an envelope, not as a static link input.
                if not lower.endswith(".deb"):
                    self.files.append(
                        {
                            **artifact(path, name),
                            "format": "static-archive",
                            "kind": "input",
                            "architecture": "unverified",
                            "identity": None,
                            "dwarf": False,
                            "debuglink": None,
                        }
                    )
                with open(path, "rb") as stream:
                    stream.seek(8)
                    table, seen = b"", set()
                    while header := stream.read(60):
                        if len(header) != 60 or header[58:] != b"`\n":
                            raise ValueError("invalid ar header")
                        size = int(header[48:58])
                        if size < 0:
                            raise ValueError("negative ar member size")
                        start = stream.tell()
                        raw = header[:16].decode().strip()
                        member_size = size
                        if raw == "//":
                            table = stream.read(size)
                        elif raw not in ("/", "/SYM64/"):
                            if raw.startswith("#1/"):
                                length = int(raw[3:])
                                raw = stream.read(length).rstrip(b"\0").decode()
                                member_size -= length
                                if member_size < 0:
                                    raise ValueError("ar name exceeds member size")
                            elif raw.startswith("/") and raw[1:].isdigit():
                                raw = table[int(raw[1:]) :].split(b"/\n", 1)[0].decode()
                            else:
                                raw = raw.rstrip("/")
                            # ar permits duplicate basenames: preserve member offsets in the path.
                            member = self.name(f"{start}/{raw}", seen)
                            self.member(
                                stream, member_size, name + "!/" + member, depth + 1
                            )
                        stream.seek(start + size + size % 2)
                return
            if head == b"!<thin>\n":
                self.files.append(
                    {
                        **artifact(path, name),
                        "format": "static-archive",
                        "kind": "input",
                        "architecture": "unverified",
                        "identity": None,
                        "dwarf": False,
                        "debuglink": None,
                    }
                )
                self.gap(
                    name,
                    "thin archive references external files; final link relationship unavailable",
                )
                return
            if lower.endswith(
                (".tar", ".tar.gz", ".tar.xz", ".tgz", ".txz", ".tar.bz2")
            ) or head[:2] in (b"\x1f\x8b", b"\xfd7"):
                self.containers.append({**artifact(path, name), "format": "tar"})
                with tarfile.open(path, "r|*") as archive:
                    seen = set()
                    for item in archive:
                        member = self.name(item.name, seen)
                        if item.isfile():
                            with archive.extractfile(item) as stream:
                                self.member(
                                    stream, item.size, name + "!/" + member, depth + 1
                                )
                        elif item.issym() or item.islnk():
                            self.aliases.append(
                                {"path": name + "!/" + member, "target": item.linkname}
                            )
                            self.gap(
                                name + "!/" + member,
                                "archive alias target not resolved",
                            )
                        if (
                            PurePosixPath(member).name.startswith(".wh.")
                            or member == "oci-layout"
                        ):
                            self.gap(
                                name,
                                "container layers require merged-rootfs inventory; layer listing is not a filesystem",
                            )
                return
            info = native(path)
            if info:
                self.files.append({**artifact(path, name), **info})
            elif lower.endswith(
                (
                    ".so",
                    ".dylib",
                    ".dll",
                    ".exe",
                    ".a",
                    ".lib",
                    ".pdb",
                    ".debug",
                    ".dwarf",
                    ".zst",
                    ".rpm",
                    ".msi",
                    ".pkg",
                    ".dmg",
                    ".cab",
                    ".jmod",
                    ".o",
                    ".obj",
                    ".bc",
                    ".rlib",
                )
            ):
                self.gap(
                    name, "unrecognized or unsupported native/debug/archive format"
                )
                self.gaps[-1]["artifact"] = artifact(path, name)
        except (
            ValueError,
            KeyError,
            IndexError,
            struct.error,
            EOFError,
            tarfile.TarError,
            zipfile.BadZipFile,
        ) as error:
            self.gap(name, f"inspection failed: {error}")
            self.gaps[-1]["artifact"] = artifact(path, name)

    def oci(self, path, name, depth):
        """Apply OCI whiteouts before inspecting modules; never extract archive paths."""
        self.containers.append({**artifact(path, name), "format": "oci"})
        with tarfile.open(path) as archive, tempfile.TemporaryDirectory() as temporary:
            members = {}
            for item in archive:
                key = item.name.removeprefix("./")
                if key in members:
                    raise ValueError("duplicate OCI archive member")
                members[key] = item

            def read_json(key):
                with archive.extractfile(members[key]) as stream:
                    return json.load(stream)

            def blob(descriptor):
                algorithm, value = descriptor["digest"].split(":", 1)
                if algorithm != "sha256" or not re.fullmatch("[a-f0-9]{64}", value):
                    raise ValueError("unsupported OCI digest")
                dest = Path(temporary) / value
                if not dest.exists():
                    with (
                        archive.extractfile(members["blobs/sha256/" + value]) as stream,
                        dest.open("wb") as out,
                    ):
                        shutil.copyfileobj(stream, out, 1024 * 1024)
                    if (
                        digest(dest) != value
                        or dest.stat().st_size != descriptor["size"]
                    ):
                        raise ValueError("OCI blob digest/size mismatch")
                return dest

            def image(descriptor, level):
                if level > MAX_DEPTH:
                    raise ValueError("OCI index nesting limit exceeded")
                data = json.loads(blob(descriptor).read_bytes())
                if "manifests" in data:
                    for child in data["manifests"]:
                        image(child, level + 1)
                    return
                if (
                    descriptor.get("annotations", {}).get("vnd.docker.reference.type")
                    == "attestation-manifest"
                ):
                    return
                if not data.get("layers") or "config" not in data:
                    self.gap(
                        name + "!/" + descriptor["digest"],
                        "not a supported OCI image manifest",
                    )
                    return
                config = json.loads(blob(data["config"]).read_bytes())
                if config.get("os") != "linux":
                    self.gap(
                        name + "!/" + descriptor["digest"],
                        "only Linux OCI root filesystems are supported",
                    )
                    return
                files = {}
                serial = 0
                prefix = name + "!/" + descriptor["digest"] + "/rootfs/"
                for layer in data["layers"]:
                    with tarfile.open(blob(layer), "r|*") as tar:
                        pending, removals = {}, []
                        seen = set()
                        for item in tar:
                            key = self.name(item.name.removeprefix("./"), seen)
                            p = PurePosixPath(key)
                            if p.name.startswith(".wh."):
                                target = (
                                    str(p.parent)
                                    if p.name == ".wh..wh..opq"
                                    else str(p.with_name(p.name[4:]))
                                )
                                removals.append((target, p.name == ".wh..wh..opq"))
                            elif item.isfile():
                                self.expanded += item.size
                                if self.expanded > MAX_EXPANDED:
                                    raise ValueError("archive expansion limit exceeded")
                                serial += 1
                                dest = Path(temporary) / f"file-{serial}"
                                with (
                                    tar.extractfile(item) as stream,
                                    dest.open("wb") as out,
                                ):
                                    shutil.copyfileobj(stream, out, 1024 * 1024)
                                pending[key] = (dest, None)
                            elif item.islnk():
                                target = self.name(
                                    item.linkname.removeprefix("./"), set()
                                )
                                pending[key] = (None, ("hardlink", target))
                                self.aliases.append(
                                    {"path": prefix + key, "target": target}
                                )
                            elif item.issym():
                                pending[key] = (None, item.linkname)
                        for target, opaque in removals:
                            for key in list(files):
                                if (
                                    (not opaque and key == target)
                                    or target == "."
                                    or key.startswith(target + "/")
                                ):
                                    del files[key]
                        files.update(pending)
                        # Resolve now: a later whiteout of the original does not delete
                        # this hardlink's bytes, and a later replacement cannot change them.
                        for key, (content, target) in list(files.items()):
                            if isinstance(target, tuple):
                                visited, current = {key}, target[1]
                                while current in files and current not in visited:
                                    visited.add(current)
                                    content, next_target = files[current]
                                    if content is not None or not isinstance(
                                        next_target, tuple
                                    ):
                                        break
                                    current = next_target[1]
                                if content is None:
                                    self.gap(
                                        prefix + key,
                                        "OCI hardlink target could not be resolved",
                                    )
                                    files[key] = (None, target[1])
                                else:
                                    files[key] = (content, None)
                for key, (content, target) in sorted(files.items()):
                    if content is None:
                        self.aliases.append({"path": prefix + key, "target": target})
                    else:
                        self.scan(content, prefix + key, depth + 1)

            index = read_json("index.json")
            for descriptor in index["manifests"]:
                image(descriptor, 0)

    @staticmethod
    def name(name, seen):
        path = PurePosixPath(name)
        if (
            path.is_absolute()
            or ".." in path.parts
            or "!" in name
            or "\\" in name
            or name in seen
        ):
            raise ValueError("ambiguous or unsafe archive member: " + name)
        seen.add(name)
        return path.as_posix()

    def tree(self, root, prefix):
        root = Path(root)
        if not root.exists():
            raise ValueError(f"required distribution/debug root missing: {root}")
        if root.is_file():
            self.scan(root, prefix)
            return
        for path in sorted(root.rglob("*")):
            name = prefix + "/" + path.relative_to(root).as_posix()
            if path.is_symlink():
                self.aliases.append({"path": name, "target": os.readlink(path)})
                # Symlinks aren't followed: don't traverse outside the distribution.
                target = path.resolve()
                if not target.is_relative_to(root.resolve()) or not target.exists():
                    self.gap(name, "external or dangling symlink")
            elif path.is_file():
                self.scan(path, name)


def pair(module, candidates):
    if module["format"] not in ("elf", "macho"):
        return {
            "status": "unsupported",
            "reason": "platform identity/debug pairing not implemented",
            "artifacts": [],
        }
    identity = module["identity"]
    if not identity:
        return {
            "status": "unverified",
            "reason": "module has no supported identity",
            "artifacts": [],
        }
    matches = [
        c
        for c in candidates
        if c["dwarf"]
        and c["identity"] == identity
        and c["architecture"] == module["architecture"]
        and c["format"] == module["format"]
    ]
    if module["dwarf"]:
        matches.append(module)
    matches = {c["sha256"]: c for c in matches}
    if not matches:
        return {
            "status": "missing",
            "reason": "no identity-matched DWARF artifact collected",
            "artifacts": [],
        }
    if module.get("debuglink"):
        crc_matches = {
            sha: c
            for sha, c in matches.items()
            if c["crc32"] == module["debuglink"]["crc32"]
        }
        if not crc_matches:
            return {
                "status": "unverified",
                "reason": "GNU debuglink CRC differs from identity-matched artifact",
                "artifacts": list(matches.values()),
            }
        matches = crc_matches
    # Conflicting candidates with a shared build ID must never silently pick a winner.
    if len(matches) > 1:
        return {
            "status": "unverified",
            "reason": "multiple different DWARF artifacts share identity",
            "artifacts": list(matches.values()),
        }
    return {
        "status": "identity-matched",
        "reason": "identity and architecture match; DWARF section present; address/line resolution not tested",
        "artifacts": list(matches.values()),
    }


def evidence(root, paths):
    records, dependencies = [], []
    for name in sorted(set(paths)):
        path = root / name
        if not path.is_file():
            records.append({"path": name, "status": "missing"})
            continue
        records.append({**artifact(path, name), "status": "collected"})
        if path.name in ("manifest.json", "versions.json", "frontend-seam.json"):
            # Preserve upstream provenance as evidence, without asserting module inclusion.
            records[-1]["details"] = json.loads(path.read_text())
        if path.name == "Cargo.lock":
            for package in tomllib.loads(path.read_text())["package"]:
                source = package.get("source", "")
                if source.startswith("git+"):
                    dependencies.append(
                        {
                            "name": package["name"],
                            "version": package["version"],
                            "source": source,
                            "evidence": name,
                            "relationship": "locked-input; final-module inclusion unverified",
                        }
                    )
    return records, dependencies


def collect(
    root,
    distributions,
    debug_roots,
    repository,
    metadata,
    evidence_paths=(),
    maps=(),
    commit=None,
    input_roots=(),
    links=(),
):
    root = Path(root)
    dirty = None
    if commit is None:
        commit = subprocess.check_output(
            ["git", "-C", str(root), "rev-parse", "HEAD"], text=True
        ).strip()
        dirty = bool(
            subprocess.check_output(
                [
                    "git",
                    "-C",
                    str(root),
                    "status",
                    "--porcelain",
                    "--untracked-files=no",
                ],
                text=True,
            ).strip()
        )
    if not re.fullmatch(r"[0-9a-f]{40,64}", commit):
        raise ValueError("require exact source commit")
    if not all(
        metadata.get(key)
        for key in ("release", "platform", "architecture", "configuration")
    ):
        raise ValueError(
            "release, platform, architecture and configuration metadata are required"
        )
    if any(
        key in metadata
        for key in ("repository", "commit", "revisionSource", "trackedWorktreeDirty")
    ):
        raise ValueError("metadata cannot override measured repository state")
    shipped, debug, link_inputs = Inventory(), Inventory(), Inventory()
    for name in distributions:
        label = (
            Path(name).name
            if Path(name).is_absolute() or ".." in Path(name).parts
            else name
        )
        shipped.tree(root / name, label)
    for name in debug_roots:
        if (root / name).exists():
            debug.tree(root / name, name)
        else:
            debug.gap(name, "optional debug root missing")
    for name in input_roots:
        if (root / name).exists():
            link_inputs.tree(root / name, name)
        else:
            link_inputs.gap(name, "declared link input missing")
    records, dependencies = evidence(root, evidence_paths)
    modules, inputs = [], []
    shipped_ids = {id(item) for item in shipped.files}
    debug_candidates = debug.files + [f for f in shipped.files if f["kind"] == "debug"]
    for item in shipped.files + link_inputs.files:
        if item["kind"] == "debug":
            continue
        if item["kind"] == "input":
            inputs.append(
                {
                    **item,
                    "finalModules": [],
                    "relationshipStatus": "unverified",
                    "distributed": id(item) in shipped_ids,
                }
            )
            continue
        if id(item) not in shipped_ids:
            continue
        mapping = []
        for selector, path in maps:
            if item["path"].endswith("/" + selector) or item["path"] == selector:
                record = evidence(root, [path])[0][0]
                mapping.append(
                    {**record, "association": "build-declared; contents not verified"}
                )
        modules.append(
            {
                **item,
                "debug": {
                    **pair(item, debug_candidates),
                    "format": "pdb-unverified"
                    if item["format"] == "pe"
                    else "dwarf"
                    if item["format"] in ("elf", "macho")
                    else "unverified",
                },
                "obfuscationMaps": mapping,
                "sources": {
                    "status": "unverified",
                    "reason": "build revision recorded; DWARF source paths/content not decoded",
                },
                "provenance": {
                    "status": "unverified",
                    "reason": "build repository is not proof of ownership of copied/embedded modules",
                },
            }
        )
    for input_selector, module_selector, proof in links:
        selected = [
            m
            for m in modules
            if m["path"] == module_selector or m["path"].endswith("/" + module_selector)
        ]
        selected_inputs = [
            i
            for i in inputs
            if i["path"] == input_selector
            or i["path"].startswith(input_selector + "!/")
            or i["path"].endswith("/" + input_selector)
            or ("/" + input_selector + "!/") in i["path"]
        ]
        record = evidence(root, [proof])[0][0]
        if len(selected) != 1 or not selected_inputs or record["status"] != "collected":
            link_inputs.gap(
                input_selector,
                "link declaration could not be resolved uniquely with evidence",
            )
            continue
        for item in selected_inputs:
            item["finalModules"].append(
                {
                    "path": selected[0]["path"],
                    "sha256": selected[0]["sha256"],
                    "evidence": record,
                    "relationship": "build-declared; address contribution unverified",
                }
            )
            item["relationshipStatus"] = "build-declared"
    result = {
        "schemaVersion": VERSION,
        "producer": "debuginfo.dev/symbol-coverage-v1",
        "build": {
            "repository": repository,
            "commit": commit,
            "revisionSource": "git" if dirty is not None else "build-declared",
            "trackedWorktreeDirty": dirty,
            **metadata,
        },
        "scope": {
            "distributions": [
                Path(n).name if Path(n).is_absolute() or ".." in Path(n).parts else n
                for n in distributions
            ],
            "debugRoots": list(debug_roots),
            "inputRoots": list(input_roots),
            "completeness": "unverified",
            "jit": "not-collected",
            "sourceContent": "not-collected",
        },
        "modules": modules,
        "inputs": inputs,
        "containers": shipped.containers,
        "debugArtifacts": [f for f in shipped.files + debug.files if f["dwarf"]],
        "aliases": shipped.aliases,
        "evidence": records,
        "dependencies": dependencies,
        "gaps": shipped.gaps + debug.gaps + link_inputs.gaps,
    }
    if not modules:
        result["gaps"].append(
            {
                "path": ".",
                "status": "unverified",
                "reason": "no final native modules discovered",
            }
        )
    validate(result)
    return result


def validate(manifest):
    if (
        manifest["schemaVersion"] != VERSION
        or manifest["scope"]["completeness"] != "unverified"
    ):
        raise ValueError("unsupported contract or unjustified completeness claim")
    paths = [m["path"] for m in manifest["modules"]]
    if len(paths) != len(set(paths)):
        raise ValueError("duplicate module path (overlapping distribution roots)")
    for module in manifest["modules"]:
        if module["debug"]["status"] == "identity-matched":
            if len(module["debug"]["artifacts"]) != 1:
                raise ValueError("ambiguous debug pairing")
            for debug in module["debug"]["artifacts"]:
                if (
                    not module["identity"]
                    or debug["identity"] != module["identity"]
                    or not debug["dwarf"]
                    or debug["format"] != module["format"]
                    or debug["architecture"] != module["architecture"]
                ):
                    raise ValueError("unjustified debug identity pairing")
        if not re.fullmatch(r"[a-f0-9]{64}", module["sha256"]):
            raise ValueError("missing exact distributed byte hash")
    measured = {(m["path"], m["sha256"]) for m in manifest["modules"]}
    for item in manifest["inputs"]:
        for target in item["finalModules"]:
            if (target["path"], target["sha256"]) not in measured:
                raise ValueError(
                    "link relationship does not identify a measured final module"
                )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--repository", required=True)
    parser.add_argument("--distribution", action="append", required=True)
    parser.add_argument("--debug-root", action="append", default=[])
    parser.add_argument("--input-root", action="append", default=[])
    parser.add_argument(
        "--link",
        action="append",
        default=[],
        help="input suffix=final module suffix=build evidence path",
    )
    parser.add_argument("--evidence", action="append", default=[])
    parser.add_argument(
        "--map",
        action="append",
        default=[],
        help="module suffix=repository-relative map path",
    )
    parser.add_argument("--metadata", required=True, help="JSON build metadata")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    manifest = collect(
        args.root,
        args.distribution,
        args.debug_root,
        args.repository,
        json.loads(args.metadata),
        args.evidence,
        [entry.split("=", 1) for entry in args.map],
        input_roots=args.input_root,
        links=[entry.split("=", 2) for entry in args.link],
    )
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(
        f"Symbol coverage: {len(manifest['modules'])} modules, {len(manifest['inputs'])} inputs, {len(manifest['gaps'])} inspection gaps -> {args.out}"
    )


if __name__ == "__main__":
    main()
