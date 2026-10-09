#!/usr/bin/env python3
"""Resolve public artifacts and exercise the standalone JVM example in fresh caches."""
import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import subprocess
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
VERSION = "0.3.0"
CENTRAL = "https://repo.maven.apache.org/maven2/dev/elide/bemo"


def run(command, cwd, env, log):
  with log.open("w") as output:
    subprocess.run(command, cwd=cwd, env=env, stdout=output, stderr=subprocess.STDOUT, check=True)


def serve(command, cwd, env, log):
  with log.open("w") as output:
    process = subprocess.Popen(command, cwd=cwd, env=env, stdout=output, stderr=subprocess.STDOUT,
                               start_new_session=True)
    try:
      deadline = time.monotonic() + 60
      while True:
        if process.poll() is not None:
          raise RuntimeError(f"Server exited: see {log}")
        try:
          connection = http.client.HTTPConnection("127.0.0.1", 8080, timeout=2)
          for _ in range(2):
            connection.request("GET", "/plaintext")
            response = connection.getresponse()
            if response.status != 200 or response.read() != b"Hello, World!":
              raise RuntimeError("Incorrect quickstart response")
            if response.getheader("Content-Type") != "text/plain":
              raise RuntimeError("Incorrect quickstart content type")
          connection.close()
          break
        except OSError:
          if time.monotonic() >= deadline:
            raise RuntimeError("Quickstart readiness timed out")
          time.sleep(.1)
    finally:
      os.killpg(process.pid, signal.SIGTERM)
      try:
        process.wait(timeout=15)
      except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()
        raise RuntimeError("Quickstart shutdown timed out")
  deadline = time.monotonic() + 15
  while "Bemo stopped; workload released" not in log.read_text() and time.monotonic() < deadline:
    time.sleep(.1)
  text = log.read_text()
  if "Bemo FFM: DriverSelection[" not in text or "Bemo stopped; workload released" not in text:
    raise RuntimeError(f"Missing binding/shutdown evidence: see {log}")


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument("--output", type=Path, required=True, help="New evidence directory")
  parser.add_argument("--builder", choices=("maven", "gradle", "both"), default="both")
  args = parser.parse_args()
  classifier = {("Darwin", "arm64"): "osx-aarch64",
                ("Linux", "x86_64"): "linux-x86_64-gnu"}.get((platform.system(), platform.machine()))
  if classifier is None:
    parser.error("Qualification requires macOS ARM64 or glibc Linux x86-64")
  output = args.output.resolve()
  output.mkdir(parents=True, exist_ok=False)
  project = output / "project"
  shutil.copytree(ROOT / "examples/quickstart", project,
                  ignore=shutil.ignore_patterns("target", "build", ".gradle", "runtime.classpath"))
  env = {key: value for key, value in os.environ.items()
         if key not in ("JAVA_TOOL_OPTIONS", "JDK_JAVA_OPTIONS", "MAVEN_OPTS", "GRADLE_OPTS", "CLASSPATH")
         and not any(word in key.upper() for word in ("TOKEN", "PASSWORD", "SECRET"))}
  env["GRADLE_USER_HOME"] = str(output / "gradle-home")
  settings = output / "settings.xml"
  settings.write_text('<settings xmlns="http://maven.apache.org/SETTINGS/1.2.0" />\n')
  artifacts = []
  for module in ("bemo-api", "bemo-ffm", "bemo-netty", "bemo-native-image"):
    names = [f"{module}-{VERSION}.pom", f"{module}-{VERSION}.jar"]
    if module in ("bemo-ffm", "bemo-native-image"):
      names += [f"{module}-{VERSION}-{target}.jar" for target in ("linux-x86_64-gnu", "osx-aarch64")]
    for name in names:
      url = f"{CENTRAL}/{module}/{VERSION}/{name}"
      with urllib.request.urlopen(url, timeout=60) as response:
        data = response.read()
      artifacts.append({"url": url, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
  commands = []
  results = {"version": VERSION, "platform": platform.platform(), "classifier": classifier,
             "java": subprocess.check_output(["java", "-version"], env=env, stderr=subprocess.STDOUT, text=True).strip(),
             "artifacts": artifacts, "commands": commands, "results": {}}
  (output / "evidence.json").write_text(json.dumps(results, indent=2) + "\n")
  builders = ("maven", "gradle") if args.builder == "both" else (args.builder,)
  for builder in builders:
    if builder == "maven":
      command = ["mvn", "-B", "-s", str(settings), "-gs", str(settings),
                 f"-Dmaven.repo.local={output / 'm2'}", f"-Dbemo.classifier={classifier}",
                 "package", "dependency:build-classpath", "-Dmdep.outputFile=target/runtime.classpath"]
      commands.append(command)
      run(command, project, env, output / "maven-build.log")
      cp = str(project / "target/classes") + os.pathsep + (project / "target/runtime.classpath").read_text().strip()
      launch = ["java", "--enable-native-access=ALL-UNNAMED", "-cp", cp,
                "dev.elide.bemo.examples.quickstart.Application"]
    else:
      command = ["./gradlew", "--no-daemon", f"-Pbemo.classifier={classifier}", "installDist"]
      commands.append(command)
      run(command, project, env, output / "gradle-build.log")
      launch = [str(project / "build/install/bemo-quickstart/bin/bemo-quickstart")]
    commands.append(launch)
    serve(launch, project, env, output / f"{builder}-server.log")
    results["results"][builder] = "passed"
    (output / "evidence.json").write_text(json.dumps(results, indent=2) + "\n")
    print(f"{builder}: passed ({classifier})", flush=True)


if __name__ == "__main__":
  main()
