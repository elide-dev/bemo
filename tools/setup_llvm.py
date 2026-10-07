#!/usr/bin/env python3
"""Install the pinned checksummed Elide native toolchain locally."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

import seam


def digest(path):
  with Path(path).open('rb') as stream:
    return hashlib.file_digest(stream, 'sha256').hexdigest()


def extract(archive, checksum, destination):
  if digest(archive) != checksum:
    raise RuntimeError('LLVM distribution checksum mismatch')
  destination = Path(destination)
  destination.parent.mkdir(parents=True, exist_ok=True)
  with tempfile.TemporaryDirectory(prefix='llvm-extract-', dir=destination.parent) as tmp:
    with tarfile.open(archive, 'r:xz') as source:
      source.extractall(tmp, filter='data')
    roots = list(Path(tmp).iterdir())
    if len(roots) != 1 or not roots[0].is_dir():
      raise RuntimeError('Expected one LLVM distribution root')
    shutil.rmtree(destination, ignore_errors=True)
    shutil.move(str(roots[0]), destination)


def install():
  pins = json.loads((seam.ROOT / 'tools/versions.json').read_text())['llvm']
  triple = seam.target_triple()
  if triple not in pins['archives']:
    raise RuntimeError(f'No pinned LLVM distribution for {triple}; configure LLVM_BIN manually')
  archive_pin = pins['archives'][triple]
  destination = seam.ROOT / 'build/tools' / f'elide-toolchain-{pins["toolchain_version"]}-{triple}'
  marker = destination / '.installed-sha256'
  if not marker.exists() or marker.read_text().strip() != archive_pin['sha256']:
    destination.parent.mkdir(parents=True, exist_ok=True)
    archive = Path(str(destination) + '.tar.xz')
    if not archive.exists() or digest(archive) != archive_pin['sha256']:
      print(f'Downloading pinned Elide toolchain {pins["toolchain_version"]} for {triple}', flush=True)
      temporary = archive.with_suffix('.download')
      try:
        with urllib.request.urlopen(archive_pin['url'], timeout=600) as source, temporary.open('wb') as output:
          shutil.copyfileobj(source, output)
        if digest(temporary) != archive_pin['sha256']:
          raise RuntimeError('LLVM distribution checksum mismatch')
        temporary.replace(archive)
      finally:
        temporary.unlink(missing_ok=True)
    extract(archive, archive_pin['sha256'], destination)
    for relative in ('bin/clang', 'bin/clang++', 'bin/llvm-config', 'bin/llvm-ar', 'bin/llvm-ranlib',
                     'bin/llvm-dis', 'lib/cmake/llvm/LLVMConfig.cmake', 'include/llvm/IR/Module.h'):
      if not (destination / relative).is_file():
        raise RuntimeError(f'Pinned LLVM distribution lacks {relative}')
    marker.write_text(archive_pin['sha256'] + '\n')
  binary_dir = destination / 'bin'
  target = 'arm64-apple-darwin' if triple == 'aarch64-apple-darwin' else triple
  native_env = json.loads(subprocess.check_output(
      [binary_dir / 'elide-toolchain', 'env', '--target', target, '--format', 'json'], text=True))
  native_env['LLVM_BIN'] = str(binary_dir)
  os.environ.update(native_env)
  import bitcode
  bitcode.toolchain()
  if os.environ.get('GITHUB_PATH'):
    with Path(os.environ['GITHUB_PATH']).open('a') as output:
      output.write(str(binary_dir) + '\n')
    with Path(os.environ['GITHUB_ENV']).open('a') as output:
      for key, value in native_env.items():
        output.write(key + '=' + value + '\n')
  print(f'Installed LLVM tools: {binary_dir}')
  return binary_dir


if __name__ == '__main__':
  install()
