"""Guard against misleading README charts when benchmark inputs change."""
import copy
import json
from pathlib import Path
import tempfile
import unittest

import plot_bench as charts


class PerformanceChartTest(unittest.TestCase):
  def setUp(self):
    self.meta = json.loads(charts.DEFAULT.read_text())
    self.rows = json.loads((charts.DEFAULT.parent / self.meta["summary"]).read_text())

  def load(self, rows):
    with tempfile.TemporaryDirectory() as directory:
      root = Path(directory)
      (root / self.meta["summary"]).write_text(json.dumps(rows))
      provenance = root / "provenance.json"
      provenance.write_text(json.dumps(self.meta))
      return charts.load(provenance)

  def test_archived_jdk_tls_evidence_still_loads(self):
    meta, groups = charts.load(charts.DEFAULT.parent / "provenance-jdk.json")
    self.assertFalse(meta.get("native_netty_baseline", False))
    self.assertEqual(len(groups), 24)

  def test_raw_samples_are_authoritative(self):
    expected = charts.values(self.load(self.rows)[1], charts.CASES[0], "bemo", "requests_per_second")
    for row in self.rows:
      row["median_requests_per_second"] = -1
    meta, groups = self.load(self.rows)
    self.assertEqual(len(groups), len(meta["cases"]) * 2)
    self.assertEqual(charts.values(groups, charts.CASES[0], "bemo", "requests_per_second"), expected)
    self.assertEqual(len(meta["sha256"]), 64)

  def test_missing_cases_and_insufficient_samples_are_rejected(self):
    with self.assertRaisesRegex(ValueError, "Incomplete"):
      self.load(self.rows[:-1])
    self.rows[0]["samples"] = self.rows[0]["samples"][:1]
    with self.assertRaisesRegex(ValueError, "three samples"):
      self.load(self.rows)

  def test_mixed_environment_and_backends_are_rejected(self):
    for field, value, message in (("clients", 8, "Mixed benchmark"),
                                  ("commit", "wrong", "Mixed benchmark"),
                                  ("driver", "epoll", "Mixed transport"),
                                  ("auto_fallback", True, "AUTO fallback"),
                                  ("http_provider", "netty", "Unexpected benchmark stack")):
      with self.subTest(field=field):
        rows = copy.deepcopy(self.rows)
        rows[0]["samples"][1][field] = value
        with self.assertRaisesRegex(ValueError, message):
          self.load(rows)

  def test_gzip_provider_is_explicit_and_consistent(self):
    rows = copy.deepcopy(self.rows)
    gzip = next(row for row in rows if row['transport'] == 'bemo' and '-gzip-' in row['case'])
    gzip['samples'][0]['gzip_provider'] = 'unknown'
    with self.assertRaisesRegex(ValueError, 'Unexpected gzip'):
      self.load(rows)
    gzip['samples'][0]['gzip_provider'] = ('zlib-rs' if gzip['samples'][1]['gzip_provider'] == 'java.util.zip' else 'java.util.zip')
    with self.assertRaisesRegex(ValueError, 'Mixed gzip'):
      self.load(rows)

  def test_gzip_level_is_valid_and_consistent_within_each_stack(self):
    rows = copy.deepcopy(self.rows)
    gzip = next(row for row in rows if row['transport'] == 'bemo' and '-gzip-' in row['case'])
    gzip['samples'][0]['gzip_level'] = 10
    with self.assertRaisesRegex(ValueError, 'Invalid gzip level'):
      self.load(rows)
    gzip['samples'][0]['gzip_level'] = 3 if gzip['samples'][1].get('gzip_level', 6) != 3 else 1
    with self.assertRaisesRegex(ValueError, 'Mixed gzip levels'):
      self.load(rows)

  def test_extended_payload_matrix_is_complete_and_legacy_archives_still_load(self):
    meta, groups = self.load(self.rows)
    self.assertIn(len(groups), (16, 24))
    if len(groups) == 16:
      for row in list(self.rows):
        if row['case'].endswith('-65536'):
          extended = copy.deepcopy(row)
          extended['case'] = extended['case'].replace('-65536', '-131072')
          for sample in extended['samples']:
            sample['case'] = extended['case']
            sample['payload_bytes'] = 131072
          self.rows.append(extended)
    meta, groups = self.load(self.rows)
    self.assertEqual(len(groups), 24)
    self.assertEqual(meta['payload_sizes'], [1024, 65536, 131072])
    incomplete = [row for row in self.rows if not (row['case'] == 'tls-gzip-131072' and row['transport'] == 'bemo')]
    with self.assertRaisesRegex(ValueError, 'Incomplete'):
      self.load(incomplete)

  def test_matched_compression_rejects_different_levels(self):
    self.meta["matched_compression"] = True
    rows = copy.deepcopy(self.rows)
    for row in rows:
      for sample in row["samples"]:
        if sample["gzip"]:
          sample["gzip_level"] = 1 if row["transport"] == "bemo" else 6
    with self.assertRaisesRegex(ValueError, "Unmatched gzip levels"):
      self.load(rows)

  def test_matched_tls_requires_protocol_and_cipher(self):
    self.meta["matched_tls"] = True
    rows = copy.deepcopy(self.rows)
    for row in rows:
      for sample in row["samples"]:
        if sample["tls"]:
          sample.update(load_generator_tls_protocol="TLSv1.3", load_generator_tls_cipher="TLS_AES_128_GCM_SHA256")
    self.load(rows)
    row = next(r for r in rows if r["case"].startswith("tls-"))
    row["samples"][0]["load_generator_tls_cipher"] = "TLS_AES_256_GCM_SHA384"
    with self.assertRaisesRegex(ValueError, "Unmatched TLS"):
      self.load(rows)

  def test_native_baseline_requires_actual_channel_and_tcnative(self):
    self.meta["native_netty_baseline"] = True
    for row in self.rows:
      if row["transport"] == "bemo":
        continue
      for sample in row["samples"]:
        sample["server_channel"] = "io.netty.channel.epoll.EpollServerSocketChannel"
        if sample["tls"]:
          sample.update(tls_provider="openssl", tls_implementation="BoringSSL")
    self.load(self.rows)
    tls = next(row for row in self.rows if row["transport"] != "bemo" and row["case"].startswith("tls-"))
    for field, value, message in (("tls_provider", "jdk", "Unexpected TLS provider"),
                                  ("tls_implementation", None, "baseline evidence"),
                                  ("server_channel", "io.netty.channel.socket.nio.NioServerSocketChannel", "baseline evidence")):
      with self.subTest(field=field):
        rows = copy.deepcopy(self.rows)
        selected = next(row for row in rows if row["case"] == tls["case"] and row["transport"] == tls["transport"])
        selected["samples"][0][field] = value
        with self.assertRaisesRegex(ValueError, message):
          self.load(rows)

  def test_invalid_metrics_are_rejected(self):
    for value in (None, 0, -1, float("nan"), float("inf")):
      with self.subTest(value=value):
        rows = copy.deepcopy(self.rows)
        rows[0]["samples"][0]["peak_rss_bytes"] = value
        with self.assertRaisesRegex(ValueError, "Invalid benchmark metric"):
          self.load(rows)


class FrameworkChartTest(unittest.TestCase):
  def setUp(self):
    self.evidence = json.loads(charts.FRAMEWORK_DATA.read_text())

  def load(self):
    with tempfile.TemporaryDirectory() as directory:
      source = Path(directory) / "framework.json"
      source.write_text(json.dumps(self.evidence))
      return charts.load_frameworks(source)

  def test_archived_two_framework_matrix_still_loads(self):
    source = charts.ROOT / "docs/performance/data/framework-unclemax-level1.json"
    sha, groups = charts.load_frameworks(source)
    self.assertEqual(len(groups), 40)
    self.assertEqual(sum(len(samples) for samples in groups.values()), 120)

  def test_all_frameworks_runtimes_and_endpoints_use_raw_samples(self):
    for run in self.evidence["workloads"].values():
      run["summary"] = []
    sha, groups = self.load()
    self.assertEqual(len(sha), 64)
    count = len(self.evidence.get("frameworks", ("spring-boot", "micronaut"))) * 20
    self.assertEqual(len(groups), count)
    self.assertEqual(sum(len(samples) for samples in groups.values()), count * 3)

  def test_declared_ktor_matrix_and_native_baseline_are_required(self):
    self.evidence.update(frameworks=["spring-boot", "micronaut", "ktor"], native_netty_baseline=True)
    for run in self.evidence["workloads"].values():
      has_ktor = any(sample["framework"] == "ktor" for sample in run["samples"])
      for sample in list(run["samples"]):
        if sample["framework"] == "micronaut" and not has_ktor:
          ktor = copy.deepcopy(sample)
          ktor["framework"] = "ktor"
          run["samples"].append(ktor)
      for sample in run["samples"]:
        if sample["transport"] == "netty":
          sample.update(driver="epoll", server_channel="io.netty.channel.epoll.EpollServerSocketChannel")
          if sample["tls"]:
            sample.update(tls_provider="netty-tcnative", tls_implementation="BoringSSL")
    for run in self.evidence["workloads"].values():
      run["environment"]["arguments"] = dict(builder="elide", warmup=20, duration=20, samples=3, connections=64, threads=4, server_cpus="12-17", client_cpus="18-21")
    self.assertEqual(len(self.load()[1]), 60)
    stock = next(s for s in self.evidence["workloads"]["tls"]["samples"] if s["transport"] == "netty")
    stock["tls_provider"] = "jdk"
    with self.assertRaisesRegex(ValueError, "tcnative framework"):
      self.load()
    stock["tls_provider"] = "netty-tcnative"
    arguments = self.evidence["workloads"]["tls"]["environment"]["arguments"]
    arguments["duration"] = 10
    with self.assertRaisesRegex(ValueError, "measurement settings"):
      self.load()
    arguments["duration"] = 20
    stock["server_channel"] = "io.netty.channel.socket.nio.NioServerSocketChannel"
    with self.assertRaisesRegex(ValueError, "native Netty framework"):
      self.load()

  def test_missing_and_duplicate_repetitions_are_rejected(self):
    samples = self.evidence["workloads"]["tls"]["samples"]
    removed = samples.pop()
    with self.assertRaisesRegex(ValueError, "Incomplete framework"):
      self.load()
    samples.append(removed)
    removed["repetition"] = 0
    with self.assertRaisesRegex(ValueError, "Incomplete framework"):
      self.load()

  def test_mismatched_policy_and_response_errors_are_rejected(self):
    sample = self.evidence["workloads"]["tls-compression"]["samples"][0]
    for field, value, message in (("gzip_level", 6, "compression level"),
                                  ("tls_cipher", "TLS_AES_256_GCM_SHA384", "TLS protocol or cipher"),
                                  ("invalid_responses", 1, "response errors"),
                                  ("driver_fallback_log", ["fallback"], "backend fallback")):
      with self.subTest(field=field):
        original = sample[field]
        sample[field] = value
        with self.assertRaisesRegex(ValueError, message):
          self.load()
        sample[field] = original


class FocusedChartTest(unittest.TestCase):
  def setUp(self):
    self.evidence = json.loads(charts.FOCUSED_DATA.read_text())

  def load(self):
    with tempfile.TemporaryDirectory() as directory:
      source = Path(directory) / "focused.json"
      source.write_text(json.dumps(self.evidence))
      return charts.load_focused(source)

  def test_measured_pairs_load(self):
    _, groups = self.load()
    self.assertEqual(len(groups), 12)
    self.assertTrue(all(len(samples) == 3 for samples in groups.values()))

  def test_missing_and_duplicate_pairs_are_rejected(self):
    self.evidence["samples"].pop()
    with self.assertRaisesRegex(ValueError, "Incomplete focused"):
      self.load()
    self.evidence["samples"].append(copy.deepcopy(self.evidence["samples"][0]))
    with self.assertRaisesRegex(ValueError, "duplicate focused"):
      self.load()

  def test_jdk_tls_and_mismatched_affinity_are_rejected(self):
    entry = next(e for e in self.evidence["samples"] if e["case"].startswith("tls-") and e["metrics"]["transport"] == "epoll")
    sample = entry["metrics"]
    for field, value, message in (("tls_implementation", "JDK", "native Netty/tcnative"),
                                  ("server_cpus", "1-4", "affinity")):
      with self.subTest(field=field):
        original = sample[field]
        sample[field] = value
        with self.assertRaisesRegex(ValueError, message):
          self.load()
        sample[field] = original


if __name__ == "__main__":
  unittest.main()
