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
  native_baseline = meta.get("native_netty_baseline", False)
  matched_fields = (*MATCHED, "server_cpus", "client_cpus") if native_baseline else MATCHED
  comparator_tls = "openssl" if native_baseline else "jdk"
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
      current = tuple(sample.get(field) for field in matched_fields if field != "load_generator_tls_provider")
      if any(sample.get(field) is None for field in matched_fields):
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
      if tls and meta.get("matched_tls") and (sample.get("load_generator_tls_protocol"), sample.get("load_generator_tls_cipher")) != ("TLSv1.3", "TLS_AES_128_GCM_SHA256"):
        raise ValueError("Unmatched TLS protocol or cipher")
      if sample["load_generator_tls_provider"] != ("jdk" if tls else "none"):
        raise ValueError("Unexpected load-generator TLS provider")
      if sample["tls_provider"] != (("native" if transport == "bemo" else comparator_tls) if tls else "none"):
        raise ValueError("Unexpected TLS provider")
      if native_baseline and transport != "bemo":
        channel = "io.netty.channel." + transport + "." + ("Epoll" if transport == "epoll" else "KQueue") + "ServerSocketChannel"
        if sample["driver"] != transport or sample.get("server_channel") != channel or (tls and sample.get("tls_implementation") != "BoringSSL"):
          raise ValueError("Missing native Netty/tcnative baseline evidence")
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
  if meta.get("matched_compression") and gzip_levels["bemo"] != gzip_levels["netty"]:
    raise ValueError("Unmatched gzip levels across compared stacks")
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
    fig.text(.04, .075, f"{meta['date']} · {meta['runner']} · {'parent' if meta.get('working_tree') else 'commit'} {meta['commit'][:7]} · "
             f"{meta['samples']} paired samples" + (" · working tree" if meta.get("working_tree") else ""), fontsize=9, color=MUTED)
    fig.text(.04, .043, f"Bemo: Native Image −O3 / native HTTP / Rustls + AWS-LC / {meta['backend']}   "
             f"Netty: OpenJDK / native {meta['comparator']} / {'tcnative BoringSSL' if meta.get('native_netty_baseline') else 'JDK TLS'}", fontsize=9, color=MUTED)
    fig.text(.04, .013, f"Per-response application gzip: Bemo {meta['gzip_provider']} level {meta['gzip_level']} / Netty compressor level {meta['netty_gzip_level']}", fontsize=8, color=MUTED)
    return fig, axes[0]

  def save(fig, name, description):
    name = f"{Path(name).stem}-{meta['sha256'][:12]}.svg"
    fig.savefig(output / name, format="svg", facecolor=BG,
                metadata={"Date": None, "Creator": "tools/plot_bench.py", "Title": description,
                          "Description": f"Source SHA-256: {meta['sha256']}. {meta['run_url']}"})
    svg = output / name
    svg.write_text("\n".join(line.rstrip() for line in svg.read_text().splitlines()) + "\n")
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
    if len(sizes) > 2:
      # Keep neighboring 64/128 KiB labels apart on the logarithmic axis.
      ax.get_xticklabels()[-2].set_horizontalalignment("right")
      ax.get_xticklabels()[-1].set_horizontalalignment("left")
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


FRAMEWORK_DATA = ROOT / "docs/performance/data/native-frameworks-20261007.json"
FRAMEWORK_WORKLOADS = ("plaintext", "payload", "compression", "tls", "tls-compression")
FRAMEWORK_LABELS = ("HTTP · 13 B", "HTTP · 128 KiB", "Gzip · 128 KiB", "TLS · 128 KiB", "TLS+gzip · 128 KiB")


def load_frameworks(source):
  evidence = json.loads(source.read_text())
  groups = {}
  artifacts = None
  settings = None
  host = None
  frameworks = tuple(evidence.get("frameworks", ("spring-boot", "micronaut")))
  if not frameworks or len(set(frameworks)) != len(frameworks) or any(name not in ("spring-boot", "micronaut", "ktor") for name in frameworks):
    raise ValueError("Invalid declared framework matrix")
  for workload in FRAMEWORK_WORKLOADS:
    run = evidence["workloads"][workload]
    environment = run["environment"]
    if evidence.get("native_netty_baseline"):
      arguments = environment["arguments"]
      current = tuple(arguments.get(key) for key in ("builder", "warmup", "duration", "samples", "connections", "threads", "server_cpus", "client_cpus"))
      if any(value is None for value in current) or (settings is not None and current != settings):
        raise ValueError("Mixed framework measurement settings")
      settings = current
      current_host = tuple(environment.get(key) for key in ("host", "kernel", "java", "native_image", "wrk"))
      if any(value is None for value in current_host) or (host is not None and current_host != host):
        raise ValueError("Mixed framework hosts or toolchains")
      host = current_host
    fingerprints = environment["artifact_sha256"]
    if artifacts is not None and fingerprints != artifacts:
      raise ValueError("Mixed framework application artifacts")
    artifacts = fingerprints
    for sample in run["samples"]:
      if sample["workload"] != workload:
        raise ValueError("Mixed framework workloads")
      if sample["driver_fallback_log"] or any(sample[key] for key in (
          "invalid_responses", "connect_errors", "read_errors", "write_errors", "status_errors", "timeouts")):
        raise ValueError("Framework response errors or backend fallback")
      if sample["gzip_level"] != (1 if sample["compression"] else None):
        raise ValueError("Unmatched framework compression level")
      if sample["tls"] and (sample["tls_protocol"], sample["tls_cipher"]) != ("TLSv1.3", "TLS_AES_128_GCM_SHA256"):
        raise ValueError("Unmatched framework TLS protocol or cipher")
      if not math.isfinite(sample["requests_per_second"]) or sample["requests_per_second"] <= 0:
        raise ValueError("Invalid framework throughput")
      if evidence.get("native_netty_baseline") and sample["transport"] == "netty":
        if sample.get("driver") != "epoll" or sample.get("server_channel") != "io.netty.channel.epoll.EpollServerSocketChannel":
          raise ValueError("Missing native Netty framework baseline evidence")
        if sample["tls"] and (sample.get("tls_provider"), sample.get("tls_implementation")) != ("netty-tcnative", "BoringSSL"):
          raise ValueError("Missing tcnative framework baseline evidence")
      key = sample["framework"], sample["runtime"], workload, sample["transport"]
      groups.setdefault(key, []).append(sample)
  expected = {(framework, runtime, workload, stack)
              for framework in frameworks for runtime in ("jvm", "native")
              for workload in FRAMEWORK_WORKLOADS for stack in ("bemo", "netty")}
  if set(groups) != expected or any(len(samples) != 3 or
      {s["repetition"] for s in samples} != {0, 1, 2} for samples in groups.values()):
    raise ValueError("Incomplete framework matrix: require three samples per stack")
  return hashlib.sha256(source.read_bytes()).hexdigest(), groups


def render_frameworks(source, output, png=False):
  import matplotlib
  matplotlib.use("Agg")
  import matplotlib.pyplot as plt
  from matplotlib.ticker import FuncFormatter, FixedLocator, NullLocator
  sha, groups = load_frameworks(source)
  matplotlib.rcParams.update({"font.family": "DejaVu Sans", "font.size": 11,
                             "text.color": INK, "axes.labelcolor": MUTED, "xtick.color": MUTED,
                             "ytick.color": INK, "svg.fonttype": "path", "svg.hashsalt": "bemo-framework-v1"})
  evidence = json.loads(source.read_text())
  frameworks = tuple(evidence.get("frameworks", ("spring-boot", "micronaut")))
  names = {"spring-boot": "Spring Boot", "micronaut": "Micronaut", "ktor": "Ktor"}
  fig, axes = plt.subplots(len(frameworks), 2, figsize=(16, 11.5 + 3.5 * (len(frameworks) - 2)), facecolor=BG, squeeze=False)
  fig.subplots_adjust(left=.17, right=.87, top=.80, bottom=.19, wspace=1.10, hspace=.50)
  fig.text(.04, .955, "BEMO / FRAMEWORK PERFORMANCE", fontsize=10, weight="bold", color=COLORS["bemo"])
  fig.text(.04, .902, " / ".join(names[name] for name in frameworks) + ": every endpoint", fontsize=25, weight="bold")
  fig.text(.04, .857, "Requests/sec · logarithmic scale · dots: medians; whiskers: three-sample min–max", fontsize=12, color=MUTED)
  pairs = [(framework, runtime) for framework in frameworks for runtime in ("jvm", "native")]
  for ax, (framework, runtime) in zip(axes.flat, pairs):
    ax.set_facecolor(BG)
    ax.set_title(f"{names[framework]} · {'JVM' if runtime == 'jvm' else 'Native Image −O3'}", loc="left", fontsize=14, weight="bold", pad=18)
    for stack, offset in (("bemo", -.13), ("netty", .13)):
      for index, workload in enumerate(FRAMEWORK_WORKLOADS):
        data = [s["requests_per_second"] for s in groups[framework, runtime, workload, stack]]
        median = statistics.median(data)
        ax.errorbar(median, index + offset, xerr=[[median - min(data)], [max(data) - median]],
                    fmt="o", color=COLORS[stack], markersize=6, capsize=3, linewidth=1.8)
    ax.set_xscale("log")
    upper = max(320000, max(s["requests_per_second"] for samples in groups.values() for s in samples) * 1.4)
    ax.set_xlim(1000, upper)
    ax.xaxis.set_major_locator(FixedLocator([10 ** power for power in range(3, math.floor(math.log10(upper)) + 1)]))
    ax.xaxis.set_minor_locator(NullLocator())
    ax.xaxis.set_major_formatter(FuncFormatter(lambda value, pos: f"{value / 1000:g}k"))
    ax.set_yticks(range(5), FRAMEWORK_LABELS)
    ax.set_ylim(4.55, -.55)
    ax.set_xlabel("Completed requests / second", labelpad=9)
    ax.spines[["top", "right", "left"]].set_visible(False)
    ax.spines["bottom"].set_color(GRID)
    ax.tick_params(length=0, pad=8)
    ax.set_axisbelow(True)
    ax.grid(axis="x", color=GRID)
    ax.text(1.05, 1.06, "vs stock", transform=ax.transAxes, fontsize=10, color=MUTED)
    for index, workload in enumerate(FRAMEWORK_WORKLOADS):
      medians = [statistics.median(s["requests_per_second"] for s in groups[framework, runtime, workload, stack]) for stack in ("bemo", "netty")]
      delta = medians[0] / medians[1] - 1
      ax.text(1.05, index, f"{delta:+.1%}", transform=ax.get_yaxis_transform(), va="center", fontsize=12,
              weight="bold", color=COLORS["bemo"] if delta >= 0 else COLORS["netty"])
  handles = [plt.Line2D([], [], marker="o", linestyle="", color=COLORS[stack], markersize=7) for stack in ("bemo", "netty")]
  stock = "Stock: Netty epoll / JDK gzip / tcnative BoringSSL" if evidence.get("native_netty_baseline") else "Stock: Netty NIO / JDK gzip / JDK TLS"
  fig.legend(handles, ("Bemo: io_uring / zlib-rs / Rustls", stock),
             loc="lower left", bbox_to_anchor=(.04, .115), frameon=False, ncol=2, fontsize=11)
  fig.text(.04, .096, "Unclemax · Linux Threadripper PRO 9965WX · runtime fixed within each panel · shared framework HTTP codecs", fontsize=10, color=MUTED)
  fig.text(.04, .072, "Gzip level 1 on both sides · TLS 1.3 / AES-128-GCM · 64 connections · 20 s warmup + 20 s measured", fontsize=10, color=MUTED)
  fig.text(.04, .048, "Native Image: portable x86-64-v3, no trained PGO · closed-loop loopback results · ranges are not confidence intervals", fontsize=10, color=MUTED)
  sizes = {stack: sorted({sample["wire_body_max_bytes"] for key, samples in groups.items() if key[2] == "compression" and key[3] == stack for sample in samples}) for stack in ("bemo", "netty")}
  wire = " / ".join(("Bemo" if stack == "bemo" else "stock") + " " + ", ".join(f"{size:,}" for size in sizes[stack]) + " B" for stack in ("bemo", "netty"))
  fig.text(.04, .024, "Different compression ratios: 128 KiB ASCII → " + wire + " · source " + sha[:12], fontsize=10, color=MUTED)
  output.mkdir(parents=True, exist_ok=True)
  name = output / f"framework-throughput-{sha[:12]}.svg"
  fig.savefig(name, format="svg", facecolor=BG,
              metadata={"Date": None, "Creator": "tools/plot_bench.py", "Title": " / ".join(names[name] for name in frameworks) + " JVM and Native Image throughput",
                        "Description": f"Source SHA-256: {sha}. {evidence.get('report', 'docs/performance/unclemax-matched.md')}"})
  name.write_text("\n".join(line.rstrip() for line in name.read_text().splitlines()) + "\n")
  if png:
    fig.savefig(name.with_suffix(".png"), dpi=140, facecolor=BG)
  plt.close(fig)
  print(f"Rendered framework matrix: {name.name}")


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument("--provenance", type=Path, default=DEFAULT)
  parser.add_argument("--output", type=Path, default=ROOT / "docs/performance/graphs")
  parser.add_argument("--framework-data", type=Path, default=FRAMEWORK_DATA)
  parser.add_argument("--png", action="store_true", help="Also render PNG previews")
  args = parser.parse_args()
  meta, groups = load(args.provenance)
  render(meta, groups, args.output, args.png)
  render_frameworks(args.framework_data, args.output, args.png)
  print(f"Rendered basic charts from {meta['sha256']}")


if __name__ == "__main__":
  main()
