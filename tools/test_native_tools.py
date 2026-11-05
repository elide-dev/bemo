"""Elide aliases must not intercept Cargo or Native Image's C compiler."""
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import subprocess

import native_tools


class NativeToolsTest(unittest.TestCase):
  def test_only_elide_cc_is_shadowed(self):
    with tempfile.TemporaryDirectory() as tmp:
      root = Path(tmp)
      for name in ('elide', 'native'):
        (root / name).mkdir()
      for name in ('elide/elide', 'elide/cc', 'native/clang'):
        file = root / name
        file.write_text('#!/bin/sh\n' + ('# Dist entrypoint for alias: cc\n' if name == 'elide/cc' else '') + 'exit 0\n')
        file.chmod(0o755)
      original = {'PATH': os.pathsep.join(map(str, (root / 'elide', root / 'native'))),
                  'CC': 'custom-cc', 'RUSTFLAGS': '-Cdebuginfo=1'}
      result = native_tools.environment(original, root / 'shim')
      self.assertEqual(result['PATH'], str(root / 'shim') + os.pathsep + original['PATH'])
      self.assertEqual((root / 'shim/cc').resolve(), (root / 'native/clang').resolve())
      self.assertEqual(result['CC'], original['CC'])
      self.assertEqual(result['RUSTFLAGS'], original['RUSTFLAGS'])
      self.assertNotIn('ELIDE', original)

  def test_native_path_without_elide_is_preserved(self):
    env = {'PATH': '', 'CC': 'custom-cc'}
    self.assertEqual(native_tools.environment(env, 'unused'), env)

  def test_native_cc_is_preserved(self):
    with tempfile.TemporaryDirectory() as tmp:
      cc = Path(tmp) / 'cc'
      cc.write_text('#!/bin/sh\nexit 0\n')
      cc.chmod(0o755)
      env = {'PATH': tmp}
      self.assertEqual(native_tools.environment(env, Path(tmp) / 'shim'), env)

  def test_mise_cc_shim_resolves_to_elide_alias(self):
    with tempfile.TemporaryDirectory() as tmp:
      root = Path(tmp)
      for name in ('mise', 'clang', 'elide-cc'):
        file = root / name
        file.write_text('#!/bin/sh\n# Dist entrypoint for alias: cc\n')
        file.chmod(0o755)
      (root / 'cc').symlink_to(root / 'mise')
      with patch.object(native_tools.subprocess, 'run', return_value=
                        subprocess.CompletedProcess([], 0, str(root / 'elide-cc') + '\n')):
        result = native_tools.environment({'PATH': tmp}, root / 'shim')
      self.assertEqual(result['PATH'], str(root / 'shim') + os.pathsep + tmp)
      self.assertEqual((root / 'shim/cc').resolve(), (root / 'clang').resolve())


if __name__ == '__main__':
  unittest.main()
