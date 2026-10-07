#!/usr/bin/env python3
"""Render README charts from complete, matched transport benchmark samples."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import statistics


ROOT = Path(__file__).resolve().parents[1]
DEFAULT = ROOT / "docs/performance/data/provenance.json"
CASES = [f"{tls}-{encoding}-{size}" for tls in ("plain", "tls")
         for encoding in ("identity", "gzip") for size in (1024, 65536)]
EXTENDED_CASES = [f"{tls}-{encoding}-{size}" for tls in ("plain", "tls")
                  for encoding in ("identity", "gzip") for size in (1024, 65536, 131072)]
MATCHED = ("clients", "requests", "warmup_rounds", "java_version", "host", "workload_sha256",
           "load_generator_transport", "load_generator_tls_provider", "process_scope", "socket_buffer_bytes")
BG, INK, MUTED, GRID = "#f6f8fc", "#172b45", "#52657c", "#dae3ee"
COLORS = {"bemo": "#087f8c", "netty": "#b45928"}


def load(provenance):
  meta = json.loads(provenance.read_text())
  source = provenance.parent / meta["summary"]
  rows = json.loads(source.read_text())
  cases = EXTENDED_CASES if any(row["case"].endswith("-131072") for row in rows) else CASES
  groups = {}
  cohort = None
  drivers = {}
  gzip_providers = {}
  gzip_levels = {}
  sample_count = None
  for row in rows:
    transport = row["transport"]
    if transport not in ("bemo", "epoll", "kqueue") or row["case"] not in cases:
      raise ValueError("Expected native Bemo and Netty epoll/kqueue workloads")
    key = (row["case"], "bemo" if transport == "bemo" else "netty")
    if key in groups:
      raise ValueError("Duplicate workload/transport")
    samples = row["samples"]
    if len(samples) < 3:
      raise ValueError("README charts require at least three samples per transport")
    if sample_count is None:
      sample_count = len(samples)
    if len(samples) != sample_count:
      raise ValueError("Unpaired sample counts")
    for sample in samples:
      current = tuple(sample.get(field) for field in MATCHED if field != "load_generator_tls_provider")
      if any(sample.get(field) is None for field in MATCHED):
        raise ValueError("Missing benchmark comparison metadata")
      if cohort is None:
        cohort = current
      if cohort != current or sample["commit"] != meta["commit"]:
        raise ValueError("Mixed benchmark environments or commits")
      if sample["case"] != row["case"] or sample["transport"] != transport:
        raise ValueError("Sample does not match its workload/transport")
      expected = ("native", "native-image", "capi") if transport == "bemo" else ("netty", "jvm", "netty")
      if tuple(sample.get(field) for field in ("http_provider", "runtime", "binding")) != expected:
        raise ValueError("Unexpected benchmark stack")
      tls = row["case"].startswith("tls-")
      if (sample["tls"], sample["gzip"], sample["payload_bytes"]) != (
          tls, "-gzip-" in row["case"], int(row["case"].split("-")[-1])):
        raise ValueError("Sample payload does not match workload label")
      if sample["load_generator_tls_provider"] != ("jdk" if tls else "none"):
        raise ValueError("Unexpected load-generator TLS provider")
      if sample["tls_provider"] != (("native" if transport == "bemo" else "jdk") if tls else "none"):
        raise ValueError("Unexpected TLS provider")
      provider = sample.get("gzip_provider")
      allowed = {"java.util.zip", "zlib-rs"} if transport == "bemo" else {"netty"}
      if provider not in (allowed if sample["gzip"] else {"none"}):
        raise ValueError("Unexpected gzip provider")
      if sample["gzip"] and gzip_providers.setdefault(key[1], provider) != provider:
        raise ValueError("Mixed gzip providers")
      level = sample.get("gzip_level", 6 if sample["gzip"] else 0)
      if type(level) is not int or not 0 <= level <= 9 or (not sample["gzip"] and level != 0):
        raise ValueError("Invalid gzip level")
      if sample["gzip"] and gzip_levels.setdefault(key[1], level) != level:
        raise ValueError("Mixed gzip levels")
      if sample["auto_fallback"] or sample["driver"] not in ("io-uring", "epoll", "kqueue"):
        raise ValueError("Unexpected backend or AUTO fallback")
      stack = key[1]
      if drivers.setdefault(stack, sample["driver"]) != sample["driver"]:
        raise ValueError("Mixed transport backends")
      for metric in ("requests_per_second", "latency_p99_ns", "requests", "server_cpu_ns", "peak_rss_bytes"):
        if not isinstance(sample[metric], (int, float)) or not math.isfinite(sample[metric]) or sample[metric] <= 0:
          raise ValueError("Invalid benchmark metric")
    groups[key] = samples
  if set(groups) != {(case, stack) for case in cases for stack in ("bemo", "netty")}:
    raise ValueError("Incomplete workload matrix; retain every declared workload")
  for case in cases:
    if len(groups[case, "bemo"]) != len(groups[case, "netty"]):
      raise ValueError("Unpaired sample counts")
  meta["sha256"] = hashlib.sha256(source.read_bytes()).hexdigest()
  meta["cases"] = cases
  meta["payload_sizes"] = [1024, 65536, 131072] if cases == EXTENDED_CASES else [1024, 65536]
  meta["samples"] = len(groups[CASES[0], "bemo"])
  meta["gzip_level"] = gzip_levels["bemo"]
  meta["netty_gzip_level"] = gzip_levels["netty"]
  meta["gzip_provider"] = gzip_providers["bemo"]
  meta["backend"] = groups[CASES[0], "bemo"][0]["driver"]
  meta["comparator"] = groups[CASES[0], "netty"][0]["driver"]
  return meta, groups


def values(groups, case, stack, metric):
  samples = groups[case, stack]
  if metric == "cpu":
    return [s["server_cpu_ns"] / s["requests"] / 1000 for s in samples]
  if metric == "rss":
    return [s["peak_rss_bytes"] / 2 ** 20 for s in samples]
  divisor = 1000 if metric in ("requests_per_second", "latency_p99_ns") else 1
  return [s[metric] / divisor for s in samples]


def label(case):
  tls, encoding, size = case.split("-")
  return f"{'TLS' if tls == 'tls' else 'HTTP'} · {encoding} · {int(size) // 1024} KiB"


def render(meta, groups, output, png=False):
  import matplotlib
  matplotlib.use("Agg")
  import matplotlib.pyplot as plt
  from matplotlib.ticker import FuncFormatter
  matplotlib.rcParams.update({"font.family": "DejaVu Sans", "font.size": 11,
                             "text.color": INK, "axes.labelcolor": MUTED, "xtick.color": MUTED,
                             "ytick.color": INK, "svg.fonttype": "path", "svg.hashsalt": "bemo-bench-v1"})
  cases = meta["cases"]
  sizes = meta["payload_sizes"]
  output.mkdir(parents=True, exist_ok=True)

  def canvas(title, subtitle, panels=1, height=8.8 if len(cases) > 8 else 6.8):
    fig, axes = plt.subplots(1, panels, figsize=(12, height), facecolor=BG, squeeze=False)
    fig.subplots_adjust(left=.205 if panels == 1 else .09, right=.90, top=.76, bottom=.19, wspace=.30)
    fig.text(.04, .955, "BEMO / PERFORMANCE", fontsize=10, weight="bold", color=COLORS["bemo"])
    fig.text(.04, .895, title, fontsize=24, weight="bold")
    fig.text(.04, .845, subtitle, fontsize=11, color=MUTED)
    for ax in axes[0]:
      ax.set_facecolor(BG)
      ax.spines[["top", "right", "left"]].set_visible(False)
      ax.spines["bottom"].set_color(GRID)
      ax.tick_params(length=0, pad=8)
      ax.set_axisbelow(True)
    fig.text(.04, .075, f"{meta['date']} · {meta['runner']} · commit {meta['commit'][:7]} · "
             f"{meta['samples']} paired samples", fontsize=9, color=MUTED)
    fig.text(.04, .043, f"Bemo: Native Image −O3 / native HTTP / Rustls + AWS-LC / {meta['backend']}   "
             f"Netty: OpenJDK / native {meta['comparator']} / JDK TLS", fontsize=9, color=MUTED)
    fig.text(.04, .013, f"Per-response application gzip: Bemo {meta['gzip_provider']} level {meta['gzip_level']} / Netty compressor level {meta['netty_gzip_level']}", fontsize=8, color=MUTED)
    return fig, axes[0]

  def save(fig, name, description):
    fig.savefig(output / name, format="svg", facecolor=BG,
                metadata={"Date": None, "Creator": "tools/plot_bench.py", "Title": description,
                          "Description": f"Source SHA-256: {meta['sha256']}. {meta['run_url']}"})
    if png:
      fig.savefig(output / Path(name).with_suffix(".png"), dpi=140, facecolor=BG)
    plt.close(fig)

  fig, (ax,) = canvas("The full workload picture", "Steady-state requests/sec · higher is better · dots show median; whiskers show sample min–max")
  largest = 0
  for stack, offset in (("bemo", -.15), ("netty", .15)):
    for index, case in enumerate(cases):
      data = values(groups, case, stack, "requests_per_second")
      median = statistics.median(data)
      largest = max(largest, max(data))
      ax.errorbar(median, index + offset, xerr=[[median - min(data)], [max(data) - median]],
                  fmt="o", color=COLORS[stack], markersize=7, capsize=3, linewidth=2,
                  label=("Bemo" if stack == "bemo" else "OpenJDK Netty") if index == 0 else None)
  ax.set_yticks(range(len(cases)), [label(case) for case in cases])
  ax.set_ylim(len(cases) - .4, -.6)
  ax.set_xlim(0, largest * 1.10)
  ax.xaxis.set_major_formatter(FuncFormatter(lambda value, pos: f"{value:g}k"))
  ax.set_xlabel("Completed requests / second", labelpad=12)
  ax.grid(axis="x", color=GRID)
  ax.legend(frameon=False, loc="lower right")
  ax.text(1.025, 1.025, "vs Netty", transform=ax.transAxes, fontsize=9, color=MUTED)
  for index, case in enumerate(cases):
    bemo = statistics.median(values(groups, case, "bemo", "requests_per_second"))
    netty = statistics.median(values(groups, case, "netty", "requests_per_second"))
    ax.text(1.025, index, f"{bemo / netty - 1:+.1%}", transform=ax.get_yaxis_transform(),
            va="center", fontsize=11, weight="bold", color=COLORS["bemo"] if bemo >= netty else COLORS["netty"])
  save(fig, "throughput.svg", "All measured workloads: median throughput and sample ranges")

  fig, axes = canvas("Native HTTP across payload sizes", "Identity encoding · measured endpoints only; connecting lines guide the eye", panels=2)
  for ax, metric, title, unit in zip(axes, ("requests_per_second", "latency_p99_ns"),
                                   ("Throughput ↑", "p99 response latency ↓"), ("Requests/sec (thousands)", "Microseconds")):
    for stack in ("bemo", "netty"):
      for tls, style in (("plain", "-"), ("tls", "--")):
        data = [values(groups, f"{tls}-identity-{size}", stack, metric) for size in sizes]
        medians = [statistics.median(samples) for samples in data]
        name = f"{'Bemo' if stack == 'bemo' else 'Netty'} · {'TLS' if tls == 'tls' else 'HTTP'}"
        ax.errorbar([size / 1024 for size in sizes], medians, yerr=[[m - min(d) for m, d in zip(medians, data)],
                                          [max(d) - m for m, d in zip(medians, data)]],
                    color=COLORS[stack], linestyle=style, marker="o", capsize=3, linewidth=2, label=name)
    ax.set_xscale("log", base=2)
    ax.set_xticks([size / 1024 for size in sizes], [f"{size // 1024} KiB" for size in sizes])
    ax.set_ylim(bottom=0)
    ax.set_title(title, loc="left", fontsize=14, weight="bold", pad=16)
    ax.set_ylabel(unit)
    ax.set_xlabel("Response payload", labelpad=12)
    ax.grid(axis="y", color=GRID)
  axes[0].legend(frameon=False, fontsize=10, loc="upper right")
  save(fig, "payload-curves.svg", "Identity throughput and p99 at measured payload sizes")

  fig, axes = canvas("What each request costs", "Server CPU excludes the common client · memory includes both processes · lower is better", panels=2)
  fig.subplots_adjust(left=.205, right=.96, wspace=.38)
  for ax, metric, title in zip(axes, ("cpu", "rss"), ("Server CPU · µs / request", "Sum of RSS high-water marks · MiB")):
    for stack, offset in (("bemo", -.17), ("netty", .17)):
      data = [values(groups, case, stack, metric) for case in cases]
      amounts = [max(d) if metric == "rss" else statistics.median(d) for d in data]
      ax.barh([i + offset for i in range(len(cases))], amounts, height=.28, color=COLORS[stack],
              label="Bemo" if stack == "bemo" else "OpenJDK Netty")
    ax.set_yticks(range(len(cases)), [label(case) for case in cases] if metric == "cpu" else [])
    ax.set_ylim(len(cases) - .4, -.6)
    ax.set_xlim(left=0)
    ax.grid(axis="x", color=GRID)
    ax.set_title(title, loc="left", fontsize=12, weight="bold", pad=16)
  axes[1].legend(frameon=False, fontsize=10, loc="upper right", bbox_to_anchor=(1, -.10), ncol=2)
  fig.text(.04, .115, "Memory: maximum sample sum of server/client lifetime VmHWM; not a simultaneous peak or server-only RSS.",
           fontsize=9, color=MUTED)
  save(fig, "efficiency.svg", "Server CPU and whole-process memory for all measured workloads")


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument("--provenance", type=Path, default=DEFAULT)
  parser.add_argument("--output", type=Path, default=ROOT / "docs/performance/graphs")
  parser.add_argument("--png", action="store_true", help="Also render PNG previews")
  args = parser.parse_args()
  meta, groups = load(args.provenance)
  render(meta, groups, args.output, args.png)
  print(f"Rendered three charts from {meta['sha256']}")


if __name__ == "__main__":
  main()
