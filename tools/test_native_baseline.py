"""Reject missing or mismatched native comparison evidence before running any load."""
import unittest
from unittest.mock import patch

import examples
import build


class NativeBaselineTest(unittest.TestCase):
  def evidence(self, backend="epoll", tls=True):
    prefix = "Epoll" if backend == "epoll" else "KQueue"
    return (f"Netty native transport enabled ({backend})\n"
            f"Netty native channel verified (io.netty.channel.{backend}.{prefix}ServerSocketChannel)\n"
            + ("Netty TLS enabled (tcnative, BoringSSL)\n" if tls else ""))

  def test_requires_actual_channel_and_tcnative_before_tls_measurement(self):
    with patch.object(examples.platform, "system", return_value="Linux"):
      self.assertEqual(examples.baseline_evidence(self.evidence(), True)["driver"], "epoll")
      for evidence in ("", "Netty native transport enabled (epoll)\n",
                       self.evidence("kqueue"), self.evidence(tls=False)):
        with self.subTest(evidence=evidence), self.assertRaises(RuntimeError):
          examples.baseline_evidence(evidence, True)
      self.assertEqual(examples.baseline_evidence(self.evidence(tls=False))["tls_implementation"], None)

  def test_arm64_native_dependencies_use_netty_classifier_and_pinned_tcnative(self):
    for system, classifier in (("Linux", "linux-aarch_64"), ("Darwin", "osx-aarch_64")):
      with self.subTest(system=system), patch.object(build.platform, "system", return_value=system), \
           patch.object(build.platform, "machine", return_value="arm64"), \
           patch.object(build, "jar_dependency", side_effect=lambda group, artifact, version, classifier="":
                        (artifact, version, classifier)):
        jars = build.benchmark_netty()
        native = [jar for jar in jars if jar[2]]
        self.assertEqual({jar[2] for jar in native}, {classifier})
        self.assertIn(("netty-tcnative-boringssl-static", build.VERSIONS["tcnative"], classifier), native)


if __name__ == "__main__":
  unittest.main()
