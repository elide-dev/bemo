#!/usr/bin/env python3
"""The generated C ABI reference covers every public declaration."""
from pathlib import Path
import re
import unittest

import abi_reference


class AbiReferenceTest(unittest.TestCase):
  def test_every_header_declaration_is_documented(self):
    text = abi_reference.render()
    headers = "\n".join((abi_reference.ROOT / "include" / name).read_text()
                        for name, _, _ in abi_reference.HEADERS)
    declared = set(re.findall(r"\b((?:bemo|elide_transport)_[a-z0-9_]+)\s*\(", headers))
    documented = set(re.findall(r"`((?:bemo|elide_transport)_[a-z0-9_]+)`", text))
    self.assertEqual(declared - documented, set())
    for constant in re.findall(r"^#define ((?:BEMO|ELIDE)_[A-Z0-9_]+) ", headers, re.M):
      self.assertIn(f"`{constant}`", text)

  def test_comments_attach_to_following_declarations(self):
    items = abi_reference.parse("/* Leading doc. */\nint32_t a(void);\nint32_t b(uint64_t x);\n\nint32_t c(void);\n")
    groups = [payload for kind, payload in items if kind == "group"]
    self.assertEqual([[name for name, _ in group["functions"]] for group in groups], [["a", "b"], ["c"]])
    self.assertEqual(groups[0]["comment"], "Leading doc.")
    self.assertIsNone(groups[1]["comment"])


if __name__ == "__main__":
  unittest.main()
