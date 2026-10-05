#!/usr/bin/env python3
"""Advisory RPS/RSS comparison; hosted runners do not provide a stable gating noise floor."""
import argparse
import json
from pathlib import Path


def compare(current, baseline):
  def key(row):
    return (row["case"], row.get("transport", "bemo"), row.get("tls_provider", "native"))
  old = {key(row): row for row in baseline}
  messages = []
  for row in current:
    before = old.get(key(row))
    if before is None:
      continue
    # Changing the workload invalidates its baseline, even when the display name is unchanged.
    keys = ("requests", "warmup_rounds", "payload_bytes", "clients", "tls", "gzip", "host", "java_version", "workload_sha256", "requested_backend", "driver", "auto_fallback", "http_provider", "runtime", "binding", "process_scope", "load_generator_transport", "load_generator_tls_provider", "gzip_provider")
    if any(row["samples"][0].get(k) != before["samples"][0].get(k) for k in keys):
      messages.append(f"{row['case']}: workload/environment changed; baseline comparison skipped")
      continue
    rps = row["median_requests_per_second"] / before["median_requests_per_second"]
    if rps < 0.75:
      messages.append(f"{row['case']}: requests/sec fell {(1 - rps) * 100:.1f}% (advisory)")
    rss, prior = row["max_peak_rss_bytes"], before["max_peak_rss_bytes"]
    if rss is not None and prior and rss > prior * 1.20:
      messages.append(f"{row['case']}: peak RSS rose {(rss / prior - 1) * 100:.1f}% (advisory)")
  return messages


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument("current", type=Path)
  parser.add_argument("baseline", type=Path)
  args = parser.parse_args()
  for message in compare(json.loads(args.current.read_text()), json.loads(args.baseline.read_text())):
    print("::warning::" + message)


if __name__ == "__main__":
  main()
