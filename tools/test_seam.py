#!/usr/bin/env python3
"""Signature and generated-source failure contracts for the Myna integration."""
import unittest
import subprocess
import re
import tempfile
from pathlib import Path
import seam


class SeamTest(unittest.TestCase):
  def test_header_shapes(self):
    functions = seam.header_functions('uint32_t query(void);\nint32_t read(uint64_t handle, const uint8_t *data);')
    self.assertEqual(functions['query'], ('u32', []))
    self.assertEqual(functions['read'], ('i32', [('handle', 'u64'), ('data', 'ptr<u8>')]))

  def test_signature_drift(self):
    with self.assertRaisesRegex(RuntimeError, 'signature'):
      seam.verify_signatures({'query': ('u32', [])}, {'query': ('u64', [])})

  def test_missing_callback(self):
    with self.assertRaisesRegex(RuntimeError, 'coverage'):
      seam.verify_signatures({'poll': ('i32', [('callback', 'callback_t')])}, {})

  def test_all_callback_imports_are_described(self):
    functions = seam.descriptor_functions(seam.DESCRIPTOR.read_text())
    self.assertEqual(len(functions), 93)
    for suffix in ('callback', 'batch_callback'):
      self.assertIn('elide_transport_driver_poll_' + suffix, functions)

  def test_callback_signature_drift(self):
    descriptor = seam.DESCRIPTOR.read_text().replace(
        'param events type=ptr<void>', 'param events type=u64', 1)
    with self.assertRaisesRegex(RuntimeError, 'Callback.*signature'):
      seam.validate_descriptor(descriptor)

  def test_untracked_sources_do_not_enter_pinned_build(self):
    with tempfile.TemporaryDirectory() as tmp:
      root = Path(tmp)
      subprocess.run(['git', 'init', '-q', root], check=True)
      sources = root / 'src/main/java/dev/elide/myna'
      sources.mkdir(parents=True)
      tracked = sources / 'Tracked.java'
      tracked.write_text('class Tracked {}')
      subprocess.run(['git', '-C', root, 'add', tracked], check=True)
      (sources / 'Injected.java').write_text('class Injected {}')
      self.assertEqual(seam.generator_sources(root), [tracked])

  def test_empty_and_probe_pointer_metadata(self):
    descriptor = seam.DESCRIPTOR.read_text()
    for name in ('elide_transport_http_spans', 'elide_transport_engine_wrap',
                 'elide_transport_engine_unwrap', 'elide_transport_engine_info'):
      body = re.search(r'^import ' + name + r' .*?\nend$', descriptor, re.M | re.S)[0]
      pointers = [line for line in body.splitlines() if 'type=ptr<' in line]
      self.assertTrue(pointers)
      self.assertTrue(all('nullable=true' in line for line in pointers), name)

  def test_real_inventory(self):
    seam.validate_descriptor()
    seam.validate_rust_signatures()


if __name__ == '__main__':
  unittest.main()
