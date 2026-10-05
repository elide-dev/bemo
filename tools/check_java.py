#!/usr/bin/env python3
"""Run JVM static analysis with Elide's compiler and isolated analysis outputs."""
from pathlib import Path
import shutil
import subprocess
import contextlib

from build import BUILD, ELIDE, MODULES, ROOT, VERSIONS, classpath, deps, jar_dependency, netty, run, sdk, sources


def compiler():
  exports = [f"--add-exports=jdk.compiler/com.sun.tools.javac.{part}=ALL-UNNAMED"
             for part in ("api", "file", "main", "model", "parser", "processing", "tree", "util")]
  opens = [f"--add-opens=jdk.compiler/com.sun.tools.javac.{part}=ALL-UNNAMED" for part in ("code", "comp")]
  return [ELIDE, "java", "--", *exports, *opens, "com.sun.tools.javac.Main"]


def analyze(kind, inputs, output, **kwargs):
  shutil.rmtree(output, ignore_errors=True)
  output.mkdir(parents=True)
  # Processor dependencies stay off the consumer's runtime classpath.
  processors = ([jar_dependency("com.google.errorprone", "error_prone_core", VERSIONS["error_prone"], "with-dependencies"),
                 jar_dependency("com.uber.nullaway", "nullaway", VERSIONS["nullaway"]),
                 jar_dependency("io.github.eisop", "dataflow-errorprone", VERSIONS["dataflow"]),
                 jar_dependency("org.checkerframework", "dataflow-nullaway", VERSIONS["nullaway_dataflow"])]
                if kind == "error-prone" else
                [jar_dependency("org.checkerframework", "checker", VERSIONS["checker"]),
                 jar_dependency("org.checkerframework", "checker-util", VERSIONS["checker"])])
  cp = [*sdk(), *netty(), jar_dependency("org.jspecify", "jspecify", VERSIONS["jspecify"]),
        jar_dependency("org.checkerframework", "checker-qual", VERSIONS["checker"])]
  processors += [jar_dependency("org.checkerframework", "checker-qual", VERSIONS["checker"])]
  flags = (["-proc:none", "-XDcompilePolicy=simple", "--should-stop=ifError=FLOW",
            "-Xplugin:ErrorProne -Xep:NullAway:ERROR -XepOpt:NullAway:OnlyNullMarked=true -XepOpt:NullAway:JSpecifyMode=true"]
           if kind == "error-prone" else
           ["-proc:only", "-processor", "org.checkerframework.checker.regex.RegexChecker,org.checkerframework.checker.formatter.FormatterChecker"])
  run(*compiler(), "--release", VERSIONS["jvm_release"], "-Xlint:all" if kind == "error-prone" else "-Xlint:all,-processing", "-Werror", "-Xmaxerrs", "1000", "-Xmaxwarns", "1000",
      "-processorpath", classpath(processors), "-cp", classpath(cp), "-d", output, *flags, *inputs, timeout=180, **kwargs)


def verify_enforcement():
  fixtures = {
      "Nullness": ("error-prone", "[NullAway]", "String value() { return null; }"),
      "Equality": ("error-prone", "[SelfEquals]", "boolean value(String text) { return text.equals(text); }"),
      "Regex": ("checker", "required: @Regex String", 'void value() { java.util.regex.Pattern.compile("["); }'),
      "Format": ("checker", "[argument", 'String value() { return String.format("%d", "text"); }'),
  }
  directory = BUILD / "checks/fixtures"
  directory.mkdir(parents=True, exist_ok=True)
  for name, (kind, diagnostic, body) in fixtures.items():
    source = directory / f"{name}.java"
    source.write_text(f"package dev.elide.bemo.checks;\n@org.jspecify.annotations.NullMarked final class {name} {{ {body} }}\n")
    log_path = directory / f"{name}.log"
    with log_path.open("w") as log, contextlib.redirect_stdout(log):
      try:
        analyze(kind, [source], directory / name, stdout=log, stderr=subprocess.STDOUT)
      except subprocess.CalledProcessError:
        pass
      else:
        raise RuntimeError(f"{kind} accepted invalid {name} fixture")
    if diagnostic not in log_path.read_text():
      raise RuntimeError(f"{kind} failed without the expected {name} diagnostic; see {log_path}")
  print("Static analyzer rejection contracts passed")


def check():
  deps()
  inputs = []
  packages = set()
  for module in MODULES:
    for path in sources(module):
      if path.name == "package-info.java":
        package = path.read_text().split("package ", 1)[1]
        if package in packages:
          continue
        packages.add(package)
      inputs.append(path)
  for kind in ("error-prone", "checker"):
    # CF 4.3.0 crashes on signature-polymorphic MethodHandle.invokeExact.
    # Error Prone/NullAway cover all adapters; CF checks API and Netty layers.
    selected = inputs if kind == "error-prone" else [p for p in inputs if p.parts[len(ROOT.parts) + 1] in ("api", "netty")]
    analyze(kind, selected, BUILD / "checks" / kind)
  verify_enforcement()


if __name__ == "__main__":
  check()
