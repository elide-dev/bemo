#!/usr/bin/env python3
"""Checksummed LLVM distribution extraction contracts."""
import hashlib
import io
from pathlib import Path
import tarfile
import tempfile
import unittest
import setup_llvm


class SetupLLVMTest(unittest.TestCase):
  def test_digest_mismatch_never_extracts(self):
    with tempfile.TemporaryDirectory() as tmp:
      root = Path(tmp)
      archive = root / 'llvm.tar.xz'
      archive.write_bytes(b'not the pinned archive')
      with self.assertRaisesRegex(RuntimeError, 'checksum'):
        setup_llvm.extract(archive, '0' * 64, root / 'install')
      self.assertFalse((root / 'install').exists())

  def test_archive_root_is_normalized(self):
    with tempfile.TemporaryDirectory() as tmp:
      root = Path(tmp)
      archive = root / 'llvm.tar.xz'
      with tarfile.open(archive, 'w:xz') as output:
        entry = tarfile.TarInfo('LLVM-fixture/bin/clang')
        entry.size = 5
        output.addfile(entry, io.BytesIO(b'clang'))
      setup_llvm.extract(archive, hashlib.sha256(archive.read_bytes()).hexdigest(), root / 'install')
      self.assertEqual((root / 'install/bin/clang').read_bytes(), b'clang')

  def test_addon_merges_into_a_renamed_bundle_root(self):
    with tempfile.TemporaryDirectory() as tmp:
      root = Path(tmp)
      bundle = root / 'elide-toolchain-1.0-x86_64-unknown-linux-gnu'
      (bundle / 'bin').mkdir(parents=True)
      (bundle / 'bin/clang').write_bytes(b'clang')
      archive = root / 'addon.tar.xz'
      with tarfile.open(archive, 'w:xz') as output:
        entry = tarfile.TarInfo('elide-toolchain/share/sanitizers/asan.addon.json')
        entry.size = 2
        output.addfile(entry, io.BytesIO(b'{}'))
      setup_llvm.overlay(archive, hashlib.sha256(archive.read_bytes()).hexdigest(), bundle)
      self.assertEqual((bundle / 'share/sanitizers/asan.addon.json').read_bytes(), b'{}')
      self.assertEqual((bundle / 'bin/clang').read_bytes(), b'clang')
      self.assertEqual(sorted(path.name for path in root.iterdir()), ['addon.tar.xz', bundle.name])

  def test_addon_digest_mismatch_never_overlays(self):
    with tempfile.TemporaryDirectory() as tmp:
      root = Path(tmp)
      (root / 'bundle').mkdir()
      archive = root / 'addon.tar.xz'
      archive.write_bytes(b'not the pinned add-on')
      with self.assertRaisesRegex(RuntimeError, 'checksum'):
        setup_llvm.overlay(archive, '0' * 64, root / 'bundle')
      self.assertEqual(list((root / 'bundle').iterdir()), [])


if __name__ == '__main__':
  unittest.main()
