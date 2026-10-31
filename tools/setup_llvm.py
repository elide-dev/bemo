#!/usr/bin/env python3
"""Install the pinned checksummed Elide native toolchain locally."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

import seam


def digest(path):
  with Path(path).open('rb') as stream:
    return hashlib.file_digest(stream, 'sha256').hexdigest()


def unpack(archive, checksum, parent):
  """Verify an archive, extract it below parent, and return its single root."""
  if digest(archive) != checksum:
    raise RuntimeError('Toolchain distribution checksum mismatch')
  tmp = tempfile.mkdtemp(prefix='llvm-extract-', dir=parent)
  with tarfile.open(archive, 'r:*') as source:
    source.extractall(tmp, filter='data')
  roots = list(Path(tmp).iterdir())
  if len(roots) != 1 or not roots[0].is_dir():
    shutil.rmtree(tmp)
    raise RuntimeError('Expected one toolchain distribution root')
  return Path(tmp), roots[0]


def extract(archive, checksum, destination):
  destination = Path(destination)
  destination.parent.mkdir(parents=True, exist_ok=True)
  tmp, root = unpack(archive, checksum, destination.parent)
  try:
    shutil.rmtree(destination, ignore_errors=True)
    shutil.move(str(root), destination)
  finally:
    shutil.rmtree(tmp, ignore_errors=True)


def overlay(archive, checksum, destination):
  """Merge an add-on into an installed bundle whatever the bundle's directory is named.

  `elide-toolchain addon install` extracts into the bundle's parent and therefore
  requires the bundle root to be named `elide-toolchain`; this checkout versions it.
  """
  destination = Path(destination)
  tmp, root = unpack(archive, checksum, destination.parent)
  try:
    shutil.copytree(root, destination, symlinks=True, dirs_exist_ok=True)
  finally:
    shutil.rmtree(tmp, ignore_errors=True)


def fetch(url, checksum, archive):
  archive = Path(archive)
  if archive.exists() and digest(archive) == checksum:
    return archive
  print(f'Downloading {url}', file=sys.stderr, flush=True)
  temporary = archive.with_suffix('.download')
  try:
    request = urllib.request.Request(url, headers={'User-Agent': 'bemo-toolchain/1.0'})
    with urllib.request.urlopen(request, timeout=600) as source, temporary.open('wb') as output:
      shutil.copyfileobj(source, output)
    if digest(temporary) != checksum:
      raise RuntimeError('Toolchain distribution checksum mismatch')
    temporary.replace(archive)
  finally:
    temporary.unlink(missing_ok=True)
  return archive


def home(sanitizers=()):
  """Install the pinned bundle plus any requested sanitizer add-ons; return its root."""
  pins = json.loads((seam.ROOT / 'tools/versions.json').read_text())['llvm']
  triple = seam.target_triple()
  if triple not in pins['archives']:
    raise RuntimeError(f'No pinned LLVM distribution for {triple}; configure LLVM_BIN manually')
  archive_pin = pins['archives'][triple]
  destination = seam.ROOT / 'build/tools' / f'elide-toolchain-{pins["toolchain_version"]}-{triple}'
  marker = destination / '.installed-sha256'
  if not marker.exists() or marker.read_text().strip() != archive_pin['sha256']:
    destination.parent.mkdir(parents=True, exist_ok=True)
    archive = fetch(archive_pin['url'], archive_pin['sha256'], str(destination) + '.tar.xz')
    extract(archive, archive_pin['sha256'], destination)
    for relative in ('bin/clang', 'bin/clang++', 'bin/llvm-config', 'bin/llvm-ar', 'bin/llvm-ranlib',
                     'bin/llvm-dis', 'lib/cmake/llvm/LLVMConfig.cmake', 'include/llvm/IR/Module.h'):
      if not (destination / relative).is_file():
        raise RuntimeError(f'Pinned LLVM distribution lacks {relative}')
    marker.write_text(archive_pin['sha256'] + '\n')
  for sanitizer in sanitizers:
    addon = pins.get('sanitizers', {}).get(triple, {}).get(sanitizer)
    if addon is None:
      # The bundle still ships the runtime; only libc++ and C components stay uninstrumented.
      print(f'No pinned {sanitizer} add-on for {triple}; using the bundled runtime', file=sys.stderr, flush=True)
      continue
    addon_marker = destination / f'.addon-{sanitizer}-sha256'
    if addon_marker.exists() and addon_marker.read_text().strip() == addon['sha256']:
      continue
    archive = fetch(addon['url'], addon['sha256'], f'{destination}-sanitizer-{sanitizer}.tar.xz')
    overlay(archive, addon['sha256'], destination)
    addon_marker.write_text(addon['sha256'] + '\n')
  return destination


def toolchain_target(triple):
  return 'arm64-apple-darwin' if triple == 'aarch64-apple-darwin' else triple


def environment(root, triple, sanitizer=None, static=False):
  """Return `elide-toolchain env` for a target, optionally static or with a sanitizer layer."""
  command = [Path(root) / 'bin/elide-toolchain', 'env', '--target', toolchain_target(triple),
             '--format', 'json']
  if sanitizer:
    command += ['--sanitizer', sanitizer]
  if static:
    command.append('--static')
  return json.loads(subprocess.check_output(command, text=True))


def install(sanitizers=()):
  destination = home(sanitizers)
  binary_dir = destination / 'bin'
  native_env = environment(destination, seam.target_triple())
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
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument('--sanitizer', action='append', default=[], choices=('asan', 'tsan'),
                      help='Also install the pinned instrumented add-on (repeatable)')
  install(parser.parse_args().sanitizer)
