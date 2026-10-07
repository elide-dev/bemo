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


if __name__ == '__main__':
  unittest.main()
