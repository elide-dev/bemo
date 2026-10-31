#!/usr/bin/env python3
"""Provision the checksummed GraalVM build matching Bemo's SDK dependencies."""
import json
import seam
from setup_llvm import extract, fetch


def home():
  pins = json.loads((seam.ROOT / 'tools/versions.json').read_text())
  distribution = pins['graalvm']
  if distribution['version'] != pins['graalvm_sdk']:
    raise RuntimeError('GraalVM toolchain and SDK versions must agree')
  target = seam.target_triple()
  pin = distribution['archives'].get(target)
  if pin is None:
    raise RuntimeError(f'No pinned GraalVM distribution for {target}')
  destination = seam.ROOT / 'build/tools' / f'graalvm-{distribution["version"]}-{target}'
  marker = destination / '.installed-sha256'
  if not marker.exists() or marker.read_text().strip() != pin['sha256']:
    destination.parent.mkdir(parents=True, exist_ok=True)
    archive = fetch(pin['url'], pin['sha256'], str(destination) + '.tar.gz')
    extract(archive, pin['sha256'], destination)
    marker.write_text(pin['sha256'] + '\n')
  java_home = destination / 'Contents/Home' if (destination / 'Contents/Home').is_dir() else destination
  if not (java_home / 'bin/native-image').is_file():
    raise RuntimeError('Pinned GraalVM distribution lacks bin/native-image')
  return java_home


if __name__ == '__main__':
  print(home())
