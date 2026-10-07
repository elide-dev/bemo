#!/usr/bin/env python3
"""Archive content and ThinLTO failure contracts."""
import unittest
from unittest.mock import patch
import bitcode


def archive(name, data):
  header = f'{name + "/":<16}{0:<12}{0:<6}{0:<6}{644:<8}{len(data):<10}`\n'.encode()
  return b'!<arch>\n' + header + data + (b'\n' if len(data) % 2 else b'')


class BitcodeTest(unittest.TestCase):
  def test_same_major_rust_bitcode_accepts_pinned_patch_version(self):
    outputs = ['rustc 1.101\nLLVM version: 23.1.1\n', 'clang version 23.1.2\n',
               'LLD 23.1.2\n', '23.1.2\n', '2026.10.0\n']
    with patch.object(bitcode, 'tool', side_effect=lambda name: name), \
         patch.object(bitcode.subprocess, 'check_output', side_effect=outputs):
      self.assertEqual(bitcode.toolchain()['llvm'], '23.1.2')

  def test_different_rust_llvm_major_is_rejected(self):
    with patch.object(bitcode.subprocess, 'check_output', return_value='LLVM version: 22.1.1\n'):
      with self.assertRaisesRegex(RuntimeError, 'LLVM major'):
        bitcode.toolchain()

  def test_native_only_rejected(self):
    with self.assertRaisesRegex(RuntimeError, 'no LLVM bitcode'):
      bitcode.inventory(archive('native.o', b'\x7fELFabc'))

  def test_truncation_rejected(self):
    with self.assertRaisesRegex(RuntimeError, 'Truncated'):
      bitcode.inventory(archive('rust.o', b'BC\xc0\xdeabc')[:-3])

  def test_mixed_members_reported(self):
    one = archive('rust.o', b'BC\xc0\xdeabc')
    two = archive('native.o', b'\x7fELFabc')
    result = bitcode.inventory(one + two[8:])
    self.assertEqual(result['bitcode'], ['rust.o'])
    self.assertEqual(result['native'], ['native.o'])

  def test_darwin_wrapper(self):
    result = bitcode.inventory(archive('rust.o', b'\xde\xc0\x17\x0babc'))
    self.assertEqual(result['bitcode'], ['rust.o'])


if __name__ == '__main__':
  unittest.main()
