#!/usr/bin/env python3
"""Retain final-address debug peers before strip/sign, without uploading anything.

The caller owns the final rewrite boundary. A build ID alone cannot establish
that boundary: embedded ELF DWARF is compared byte for byte with the peer too.
"""

import argparse
import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import tempfile


spec = importlib.util.spec_from_file_location("coverage_inventory", Path(__file__).with_name("collect.py"))
inventory = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inventory)


def tool(name):
    for home in (os.environ.get("LLVM_BIN"), os.environ.get("ELIDE_TOOLCHAIN_HOME"), os.environ.get("LLVM_HOME")):
        if home:
            for directory in (Path(home), Path(home) / "bin"):
                candidate = directory / (name + (".exe" if os.name == "nt" else ""))
                if candidate.is_file():
                    return str(candidate)
    found = shutil.which(name)
    if not found:
        raise ValueError(f"Debug preservation requires {name}")
    return found


def run(*args):
    return subprocess.check_output([str(a) for a in args], text=True, stderr=subprocess.STDOUT)


def elf_sections(path):
    """Return section layout and contents; NOBITS has no bytes in debug peers."""
    import mmap

    with path.open("rb") as stream, mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as data:
        if data[:4] != b"\x7fELF":
            return {}
        endian = "<" if data[5] == 1 else ">"
        wide = data[4] == 2
        header = struct.unpack_from(endian + ("HHIQQQIHHHHHH" if wide else "HHIIIIIHHHHHH"), data, 16)
        offset, stride, count, strings = header[5], header[10], header[11], header[12]
        fmt = endian + ("IIQQQQIIQQ" if wide else "IIIIIIIIII")
        first = struct.unpack_from(fmt, data, offset)
        count = count or first[5]
        strings = first[6] if strings == 0xFFFF else strings
        table = [struct.unpack_from(fmt, data, offset + i * stride) for i in range(count)]
        names = table[strings]
        names = data[names[4] : names[4] + names[5]]
        result = {}
        for section in table:
            name = names[section[0] :].split(b"\0", 1)[0].decode("utf-8", "replace")
            payload = b"" if section[1] == 8 else data[section[4] : section[4] + section[5]]
            result[name] = (section[3], section[5], payload)
        return result


def code_signature(path):
    """Executable section addresses and bytes, invariant under strip/sign."""
    module = inventory.native(path)
    if module["format"] == "elf":
        # Include nonempty text sections, not debug or symbol-table bytes.
        return {name: [address, size, hashlib.sha256(payload).hexdigest()]
                for name, (address, size, payload) in elf_sections(path).items()
                if name.startswith((".text", ".init", ".fini", ".plt")) and payload}
    data = path.read_bytes()
    result = {}
    if module["format"] == "macho":
        endian = "<" if data[0] in (0xCF, 0xCE) else ">"
        wide = data[:4] in (b"\xcf\xfa\xed\xfe", b"\xfe\xed\xfa\xcf")
        commands = struct.unpack_from(endian + "I", data, 16)[0]
        position = 32 if wide else 28
        for _ in range(commands):
            command, size = struct.unpack_from(endian + "II", data, position)
            if command == (0x19 if wide else 1):
                nsects = struct.unpack_from(endian + "I", data, position + (64 if wide else 48))[0]
                start = position + (72 if wide else 56)
                for i in range(nsects):
                    section = start + i * (80 if wide else 68)
                    name = data[section : section + 16].split(b"\0")[0].decode()
                    address, length, offset = struct.unpack_from(endian + ("QQI" if wide else "III"), data, section + 32)
                    flags = struct.unpack_from(endian + "I", data, section + (64 if wide else 56))[0]
                    if flags & (0x80000000 | 0x400):
                        result[name] = [address, length, hashlib.sha256(data[offset : offset + length]).hexdigest()]
            position += size
    elif module["format"] == "pe":
        header = struct.unpack_from("<I", data, 60)[0]
        count = struct.unpack_from("<H", data, header + 6)[0]
        optional = struct.unpack_from("<H", data, header + 20)[0]
        for i in range(count):
            section = header + 24 + optional + i * 40
            name = data[section : section + 8].split(b"\0")[0].decode()
            vsize, address, size, offset = struct.unpack_from("<4I", data, section + 8)
            flags = struct.unpack_from("<I", data, section + 36)[0]
            if flags & 0x20000000:
                result[name] = [address, vsize, hashlib.sha256(data[offset : offset + size]).hexdigest()]
    if not result:
        raise ValueError(f"No executable sections to bind: {path}")
    return result


def bind(binary, out, strip=False):
    """Bind packaging/signing bytes only when executable addresses/bytes agree."""
    binary, out = Path(binary).resolve(), Path(out).resolve()
    module, code = inventory.native(binary), code_signature(binary)
    matched = False
    for path in out.rglob("preservation.json"):
        record = json.loads(path.read_text())
        if (record["binary"]["identity"], record["binary"]["architecture"]) != (module["identity"], module["architecture"]):
            continue
        if record["code"] != code:
            raise ValueError(f"Executable code changed after debug retention: {binary}")
        peer = path.parent / record["debug"]["path"]
        check_pair(binary, peer)
        if strip and module["format"] == "elf":
            run(tool("llvm-strip"), "--strip-unneeded", binary)
            run(tool("llvm-objcopy"), "--remove-section=.gnu_debuglink", binary)
            run(tool("llvm-objcopy"), "--add-gnu-debuglink=" + str(peer), binary)
        elif strip and module["format"] == "macho":
            run(tool("strip"), "-x", binary)
        if code_signature(binary) != code:
            raise ValueError("Stripping changed executable code or addresses")
        check_pair(binary, peer)
        record["binary"].update(inventory.native(binary))
        record["binary"].update(inventory.artifact(binary, binary.name))
        path.write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
        matched = True
    if not matched:
        raise ValueError(f"No retained debug record for final module: {binary}")


def dwarf_units(path):
    text = run(tool("llvm-dwarfdump"), "--debug-info", "--recurse-depth=0", path)
    if "DW_TAG_compile_unit" not in text and "DW_TAG_skeleton_unit" not in text:
        raise ValueError(f"No DWARF compilation units: {path}")
    if "DW_AT_stmt_list" not in text and "DW_UT_split_compile" not in text:
        raise ValueError(f"No DWARF line tables: {path}")
    # DWARF5 carries the ID in the unit header; DWARF4 uses a GNU attribute.
    units = re.split(r"(?=0x[0-9a-f]+: Compile Unit:)", text)
    references = []
    for unit in units:
        name = re.search(r'DW_AT_(?:GNU_)?dwo_name\s+\("(.*?)"\)', unit)
        if not name:
            continue
        identifier = re.search(r"(?:DWO_id = |DW_AT_GNU_dwo_id\s+\()(0x[0-9a-f]+)", unit)
        directory = re.search(r'DW_AT_comp_dir\s+\("(.*?)"\)', unit)
        if not identifier:
            raise ValueError(f"Split DWARF reference has no identifier: {name[1]}")
        references.append((name[1], directory[1] if directory else "", identifier[1].lower()))
    return text, references


def retain_dwarf(peer, bundle, binary_name, companion_root=None):
    text, references = dwarf_units(peer)
    (bundle / "compilation-units.txt").write_text(text, encoding="utf-8")
    companions = []
    packed = None
    if references and companion_root:
        required = {identifier for _, _, identifier in references}
        matches = {}
        for candidate in Path(companion_root).glob("*.dwp"):
            index = run(tool("llvm-dwarfdump"), "--debug-cu-index", candidate)
            available = set(re.findall(r"0x[0-9a-f]{16}\b", index.lower()))
            if required <= available:
                matches[inventory.digest(candidate)] = (candidate, index)
        if len(matches) > 1:
            raise ValueError("Different packed DWARF artifacts cover the same skeleton identifiers")
        if matches:
            packed = next(iter(matches.values()))
    if references:
        paths = []
        identifiers = {}
        for name, directory, identifier in references:
            if packed:
                continue
            source = Path(name) if Path(name).is_absolute() else Path(directory) / name
            if not source.is_file():
                raise ValueError(f"Unresolved split DWARF {name} ({identifier}); rebuild or restore its companions")
            dump = run(tool("llvm-dwarfdump"), "--debug-info", "--recurse-depth=0", source)
            ids = re.findall(r"(?:DWO_id = |DW_AT_GNU_dwo_id\s+\()(0x[0-9a-f]+)", dump)
            if identifier not in [i.lower() for i in ids]:
                raise ValueError(f"Split DWARF identifier mismatch: {source}")
            checksum = inventory.digest(source)
            if identifier in identifiers and identifiers[identifier] != checksum:
                raise ValueError(f"Different split DWARF bytes share identifier {identifier}")
            identifiers[identifier] = checksum
            paths.append(source)
        # A packaged DWP removes dependence on absolute compilation directories.
        dwp = bundle / (binary_name + ".dwp")
        if packed:
            shutil.copyfile(packed[0], dwp)
            index = packed[1]
            dwarf_units(dwp)
        else:
            response = bundle / "dwp-inputs.rsp"
            response.write_text("\n".join(json.dumps(str(p)) for p in dict.fromkeys(paths)), encoding="utf-8")
            run(tool("llvm-dwp"), "@" + str(response), "-o", dwp)
            response.unlink()
            index = run(tool("llvm-dwarfdump"), "--debug-cu-index", dwp)
        for _, _, identifier in references:
            if identifier not in index.lower():
                raise ValueError(f"Packed DWARF lacks compilation unit {identifier}")
        (bundle / "split-index.txt").write_text(index, encoding="utf-8")
        companions.append(inventory.artifact(dwp, dwp.name))
    # Record verification separately from pairing. Never suppress producer errors.
    verification = bundle / "dwarf-verify.txt"
    with verification.open("w", encoding="utf-8") as log:
        result = subprocess.run([tool("llvm-dwarfdump"), "--verify", str(peer)], stdout=log, stderr=subprocess.STDOUT)
    # Optimizing producers can emit invalid inline/location DIEs (including
    # discarded-code tombstones). Preserve the useful final-address artifact
    # with an explicit quality failure; never turn this into a verified result.
    diagnostics = verification.with_suffix(".txt.gz")
    with verification.open("rb") as source, gzip.open(diagnostics, "wb") as target:
        shutil.copyfileobj(source, target)
    verification.unlink()
    return {"status": "self-contained" if not references else "packed",
            "references": [{"name": n, "compilationDirectory": d, "id": i} for n, d, i in references],
            "companions": companions,
            "verification": {"status": "passed" if result.returncode == 0 else "producer-errors",
                             "exitCode": result.returncode,
                             "diagnostics": inventory.artifact(diagnostics, diagnostics.name)}}


def check_pair(binary, peer):
    module, debug = inventory.native(binary), inventory.native(peer)
    for field in ("identity", "architecture"):
        if not module.get(field) or module[field] != debug.get(field):
            raise ValueError(f"Debug {field} mismatch: {binary}, {peer}")
    if debug["format"] != ("pdb" if module["format"] == "pe" else module["format"]):
        raise ValueError("Debug format mismatch")
    if not (debug["dwarf"] or debug.get("pdb")):
        raise ValueError(f"Missing debug content: {peer}")
    if module["format"] == "elf":
        original, separate = elf_sections(binary), elf_sections(peer)
        if any(separate.get(name, (0, 0, b""))[2] for name in (".debug_sup", ".gnu_debugaltlink")):
            raise ValueError("Supplementary DWARF requires an explicitly retained and verified companion")
        if original[".text"][:2] != separate[".text"][:2]:
            raise ValueError("Debug text address/size differs from final executable")
        for name, section in original.items():
            if name.startswith((".debug_", ".zdebug_")) and section[2]:
                if name not in separate or section[2] != separate[name][2]:
                    raise ValueError(f"Stale debug peer: final {name} differs (possible pre-rewrite artifact)")
        link = module.get("debuglink")
        if link:
            if peer.name != link["name"] or inventory.artifact(peer, peer.name)["crc32"] != link["crc32"]:
                raise ValueError("GNU debuglink filename/CRC mismatch")
    return module


def preserve(binary, out, debug=None, mapping=None, stage="final-link", strip=False):
    binary, out = Path(binary).resolve(), Path(out).resolve()
    module = inventory.native(binary)
    if not module or module["kind"] != "module" or not module["identity"]:
        raise ValueError(f"Final module has no supported identity: {binary}")
    original_code = code_signature(binary)
    key = module["identity"]["value"] if isinstance(module["identity"], dict) else str(module["identity"])
    destination = out / (binary.name + "-" + key)
    out.mkdir(parents=True, exist_ok=True)
    if not destination.resolve().is_relative_to(out):
        raise ValueError("retention destination escapes its output root")
    # Never accept an old retention directory as proof of a cache hit. Recompute.
    with tempfile.TemporaryDirectory(prefix=".preserve-", dir=out) as temporary:
        bundle = Path(temporary)
        before = inventory.artifact(binary, binary.name)
        if module["format"] == "elf":
            link = module.get("debuglink")
            peer = bundle / (link["name"] if link else binary.name + ".debug")
            if module["dwarf"]:
                # GNU objcopy handles BOLT's PHDR layout without losing build IDs.
                script_flags = (["--set-section-flags", ".debug_gdb_scripts=contents,readonly,debug"]
                                if ".debug_gdb_scripts" in elf_sections(binary) else [])
                run(tool("objcopy"), *script_flags, "--only-keep-debug", binary, peer)
            elif debug:
                shutil.copyfile(debug, peer)
            else:
                raise ValueError(f"No embedded DWARF or explicit final debug peer: {binary}")
            check_pair(binary, peer)
            dwarf = retain_dwarf(peer, bundle, binary.name, Path(debug).parent if debug else None)
        elif module["format"] == "macho":
            dsym = bundle / (binary.name + ".dSYM")
            if debug:
                shutil.copytree(debug, dsym)
            else:
                run(tool("dsymutil"), binary, "-o", dsym)
            peer = dsym / "Contents/Resources/DWARF" / binary.name
            check_pair(binary, peer)
            dwarf = retain_dwarf(peer, bundle, binary.name)
        elif module["format"] == "pe":
            if debug:
                source = Path(debug)
            else:
                candidates = {}
                for candidate in binary.parent.glob("*.pdb"):
                    metadata = inventory.native(candidate)
                    if metadata and metadata["identity"] == module["identity"] and metadata["architecture"] == module["architecture"]:
                        candidates[inventory.digest(candidate)] = candidate
                if len(candidates) != 1:
                    raise ValueError(f"Expected one identity-matched PDB for {binary}; found {len(candidates)}")
                source = next(iter(candidates.values()))
            peer = bundle / source.name
            shutil.copyfile(source, peer)
            check_pair(binary, peer)
            # Parse DBI source counts and actual C13 line subsections, so Windows
            # builders do not need a second platform SDK just to retain a PDB.
            content = inventory.native(peer)["pdbContent"]
            dwarf = {"status": "pdb-module-records", "references": [], "companions": [], "content": content}
        else:
            raise ValueError(f"Preservation for {module['format']} requires a platform debug provider")
        maps = []
        if mapping:
            source = Path(mapping)
            value = json.loads(source.read_text())
            if not value:
                raise ValueError("Empty obfuscation map")
            target = bundle / source.name
            shutil.copyfile(source, target)
            maps.append({**inventory.artifact(target, target.name),
                         "association": "same final build action; binary/debug hashes bound in this record"})
        module = inventory.native(binary)
        record = {"version": 1, "stage": stage, "binaryBeforeStrip": before,
                  "code": code_signature(binary),
                  "binary": {**inventory.artifact(binary, binary.name), **module},
                  "debug": inventory.artifact(peer, str(peer.relative_to(bundle))),
                  "dwarf": dwarf, "maps": maps}
        # Module metadata must not overwrite the byte hash or relative name.
        record["binary"].update(inventory.artifact(binary, binary.name))
        (bundle / "preservation.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
        if destination.exists():
            shutil.rmtree(destination)
        # Promote on the same filesystem before any destructive strip step.
        # A strip/tool/disk failure must leave a durable, usable debug bundle.
        os.replace(bundle, destination)
        peer = destination / record["debug"]["path"]
        if strip and module["format"] == "elf":
            run(tool("llvm-strip"), "--strip-unneeded", binary)
            run(tool("llvm-objcopy"), "--remove-section=.gnu_debuglink", binary)
            run(tool("llvm-objcopy"), "--add-gnu-debuglink=" + str(peer), binary)
        elif strip and module["format"] == "macho":
            run(tool("strip"), "-x", binary)
        if code_signature(binary) != original_code:
            raise ValueError("Stripping changed executable code or addresses")
        bind(binary, destination)
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--debug", type=Path)
    parser.add_argument("--map", type=Path)
    parser.add_argument("--stage", default="final-link")
    parser.add_argument("--strip", action="store_true")
    parser.add_argument("--bind", action="store_true")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        if not args.debug:
            parser.error("--check requires --debug")
        check_pair(args.binary, args.debug)
    elif args.bind:
        bind(args.binary, args.out, args.strip)
    else:
        print(preserve(args.binary, args.out, args.debug, args.map, args.stage, args.strip))


if __name__ == "__main__":
    main()
