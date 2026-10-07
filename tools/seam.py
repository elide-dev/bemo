#!/usr/bin/env python3
"""Pinned Myna generation, ABI inventory checks, and callback contracts."""
import argparse
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
BUILD = ROOT / 'build/seam'
DESCRIPTOR = ROOT / 'seams/bemo.seam'
JAVA = ROOT / 'packages/native-image/src/main/java/dev/elide/bemo/svm/generated/BemoNatives.java'
CANONICAL_TARGET = 'aarch64-apple-darwin'


def run(*args, **kwargs):
  subprocess.run(list(map(str, args)), check=True, cwd=ROOT, **kwargs)


def target_triple():
  arch = {'arm64': 'aarch64', 'aarch64': 'aarch64', 'x86_64': 'x86_64'}[platform.machine()]
  suffix = {'Darwin': 'apple-darwin', 'Linux': 'unknown-linux-gnu'}[platform.system()]
  return f'{arch}-{suffix}'


def output(target=None):
  return BUILD / (target or target_triple())


def c_type(value):
  value = re.sub(r'\bconst\b', '', value).strip()
  pointers = value.count('*')
  value = value.replace('*', '').strip()
  value = {'void': 'void', 'uint8_t': 'u8', 'uint16_t': 'u16', 'uint32_t': 'u32',
           'uint64_t': 'u64', 'int32_t': 'i32', 'int64_t': 'i64'}.get(value, value)
  for _ in range(pointers):
    value = f'ptr<{value}>'
  return value


def header_functions(text):
  text = re.sub(r'/\*.*?\*/', '', text, flags=re.S)
  functions = {}
  for match in re.finditer(r'^\s*(\w+[\w\s*]*?)\s+(\w+)\(([^;{}]*)\);', text, re.M):
    returns, symbol, params = match.groups()
    arguments = []
    if params.strip() not in ('', 'void'):
      for param in params.split(','):
        item = re.fullmatch(r'(.+?)[\s*]*(\w+)\s*', param.strip())
        if not item:
          raise RuntimeError(f'Unsupported header argument: {param}')
        # Keep stars in the type rather than in the separator.
        name = item[2]
        arguments.append((name, c_type(param[:param.rfind(name)])))
    functions[symbol] = (c_type(returns), arguments)
  return functions


def descriptor_functions(text):
  functions = {}
  for match in re.finditer(r'^import (\w+) symbol=(\w+) return=(\S+)[^\n]*\n(.*?)^end$', text, re.M | re.S):
    _, symbol, returns, body = match.groups()
    params = re.findall(r'^\s*param (\w+) type=(\S+)', body, re.M)
    functions[symbol] = (returns, params)
  return functions


def verify_signatures(headers, descriptors):
  if set(headers) != set(descriptors):
    raise RuntimeError('Seam symbol coverage differs from public headers')
  for symbol, signature in headers.items():
    candidate = descriptors[symbol]
    candidate = (candidate[0], [tuple(p) for p in candidate[1]])
    if signature != candidate:
      raise RuntimeError(f'Seam signature mismatch: {symbol}: {signature} != {candidate}')


def validate_descriptor(descriptor=None):
  descriptor = descriptor if descriptor is not None else DESCRIPTOR.read_text()
  headers = {}
  for name in ('bemo.h', 'elide_transport.h'):
    headers.update(header_functions((ROOT / 'include' / name).read_text()))
  text = (ROOT / 'include/elide_transport.h').read_text()
  callback_types = re.findall(r'typedef int32_t \(\*(elide_transport_event_(?:batch_)?callback_t)\)\((.*?)\);', text, re.S)
  callbacks = {}
  for name, params in callback_types:
    parsed = header_functions(f'int32_t {name}({params});')[name]
    callbacks[name] = parsed
  declared_callbacks = {}
  for match in re.finditer(r'^callback (\w+) return=(\S+)[^\n]*\n(.*?)^end$', descriptor, re.M | re.S):
    name, returns, body = match.groups()
    declared_callbacks[name] = (returns, re.findall(r'^\s*param (\w+) type=(\S+)', body, re.M))
  if callbacks != declared_callbacks:
    raise RuntimeError('Callback function-pointer signature drift')
  for name, (returns, params) in headers.items():
    headers[name] = (returns, [(param, f'fn<{typ}>' if typ in callbacks else typ) for param, typ in params])
  records = re.findall(r'^struct (\w+) size=(\d+) align=(\d+)\n(.*?)^end$', descriptor, re.M | re.S)
  for name, _, _, body in records:
    record = re.search(r'typedef struct \{([^}]+)\}\s*' + name + r';', text)
    if not record:
      raise RuntimeError(f'Missing public record: {name}')
    fields = []
    for field in record[1].split(';'):
      field = field.strip()
      if field:
        match = re.fullmatch(r'(.*?)\b(\w+)', field)
        fields.append((match[2], c_type(match[1])))
    expected = re.findall(r'^\s*field (\w+) type=(\S+)', body, re.M)
    if fields != expected:
      raise RuntimeError(f'Record field signature drift: {name}')
  verify_signatures(headers, descriptor_functions(descriptor))


def validate_rust_signatures():
  from generate_exports import arguments
  expected = descriptor_functions(DESCRIPTOR.read_text())
  aliases = {'BufferView': 'elide_transport_buffer_view_t', 'ReceiveResult': 'elide_transport_receive_result_t',
             'c_void': 'void'}
  found = {}
  text = '\n'.join((ROOT / 'crates/bemo-ffi/src' / name).read_text() for name in ('lib.rs', 'transport.rs'))
  for match in re.finditer(r'pub (?:unsafe )?extern "C" fn (\w+)\(', text):
    name = match[1]
    if name not in expected:
      continue
    end, depth = match.end(), 1
    while depth:
      if text[end] == '(':
        depth += 1
      elif text[end] == ')':
        depth -= 1
      end += 1
    body = text[match.end():end - 1]
    returns = text[end:text.index('{', end)].strip().removeprefix('->').strip() or 'void'
    types = []
    for parameter in arguments(body):
      typ = parameter.split(':', 1)[1].strip()
      callback = re.fullmatch(r'Option<unsafe extern "C" fn\((.*)\) -> i32>', typ)
      if callback:
        abi_types = [item.strip() for item in arguments(callback[1])]
        expected_types = ['*mut c_void', '*const NativeEvent']
        callback_name = 'elide_transport_event_callback_t'
        if name == 'elide_transport_driver_poll_batch_callback':
          expected_types.append('u32')
          callback_name = 'elide_transport_event_batch_callback_t'
        if abi_types != expected_types:
          raise RuntimeError(f'Rust callback signature mismatch: {name}')
        typ = f'fn<{callback_name}>'
      pointer = re.match(r'\*(?:const|mut) (.+)', typ)
      if pointer:
        pointee = pointer[1].split('::')[-1]
        typ = 'ptr<' + aliases.get(pointee, pointee) + '>'
      types.append(typ)
    found[name] = (returns, types)
  for name, (returns, parameters) in expected.items():
    signature = (returns, [typ for _, typ in parameters])
    if found.get(name) != signature:
      raise RuntimeError(f'Rust export signature mismatch: {name}: {found.get(name)} != {signature}')


def verify_layouts(directory):
  descriptor = DESCRIPTOR.read_text()
  assertions = ['#include <stddef.h>', '#include "elide_transport.h"']
  for name, size, align, body in re.findall(r'^struct (\w+) size=(\d+) align=(\d+)\n(.*?)^end$', descriptor, re.M | re.S):
    assertions.extend([f'_Static_assert(sizeof({name}) == {size}, "{name} size");',
                       f'_Static_assert(_Alignof({name}) == {align}, "{name} alignment");'])
    for field, offset in re.findall(r'^\s*field (\w+) type=\S+ offset=(\d+)', body, re.M):
      assertions.append(f'_Static_assert(offsetof({name}, {field}) == {offset}, "{name}.{field}");')
  file = directory / 'layout-contract.c'
  file.write_text('\n'.join(assertions) + '\n')
  run(os.environ.get('CC', 'cc'), '-std=c11', '-Wall', '-Wextra', '-Werror',
      '-fsyntax-only', '-I', ROOT / 'include', file)


def generator_sources(checkout):
  tracked = subprocess.check_output(['git', '-C', str(checkout), 'ls-files',
                                    'src/main/java/dev/elide/myna'], text=True).splitlines()
  # Compile only git-tracked sources. Untracked files must not alter output
  # while provenance reports the pinned clean revision.
  return sorted(checkout / path for path in tracked
                if path.endswith('.java') and 'nativeimage' not in Path(path).parts)


def generator():
  """Compile the pinned dependency with Elide; overrides must use that same revision."""
  pins = json.loads((ROOT / 'tools/versions.json').read_text())['myna']
  checkout = Path(os.environ.get('MYNA_HOME', ROOT / 'build/tools/myna')).resolve()
  if not checkout.exists():
    run('git', 'clone', pins['repository'], checkout)
    run('git', '-C', checkout, 'checkout', '--detach', pins['revision'])
  revision = subprocess.check_output(['git', '-C', str(checkout), 'rev-parse', 'HEAD'], text=True).strip()
  if subprocess.check_output(['git', '-C', str(checkout), 'status', '--porcelain', '--untracked-files=no'], text=True).strip():
    raise RuntimeError('myna tracked sources must be clean for reproducible generation')
  if revision != pins['revision']:
    if 'MYNA_HOME' in os.environ:
      raise RuntimeError('myna revision differs from tools/versions.json; refresh the pin and regenerate')
    run('git', '-C', checkout, 'fetch', 'origin', pins['revision'])
    run('git', '-C', checkout, 'checkout', '--detach', pins['revision'])
    revision = pins['revision']
  directory = ROOT / 'build/tools/myna-classes' / revision
  if not (directory / '.compiled-tracked').exists():
    import shutil
    shutil.rmtree(directory, ignore_errors=True)
    directory.mkdir(parents=True, exist_ok=True)
    inputs = generator_sources(checkout)
    run(os.environ.get('ELIDE', 'elide'), 'javac', '--', '--release', '22', '-d', directory, *inputs)
    (directory / '.compiled-tracked').write_text(revision + '\n')
  return [os.environ.get('ELIDE', 'elide'), 'java', '--', '-cp', str(directory), 'dev.elide.myna.cli.Main']


def generate(check=False, target=None):
  validate_descriptor()
  validate_rust_signatures()
  cli = generator()
  # Common Java artifacts must be byte-identical across platform packages. Its
  # fingerprint identifies the canonical descriptor, not the consumer target.
  targets = {CANONICAL_TARGET, target or target_triple()}
  for triple in sorted(targets):
    destination = output(triple)
    destination.mkdir(parents=True, exist_ok=True)
    verify_layouts(destination)
    run(*cli, 'generate', DESCRIPTOR, '--out', destination, '--target', triple)
  source = (output(CANONICAL_TARGET) / 'BemoNatives.java').read_text()
  from build import VERSIONS, jar_dependency
  formatter = jar_dependency('com.google.googlejavaformat', 'google-java-format', VERSIONS['java_format'], 'all-deps')
  with tempfile.TemporaryDirectory(prefix='bemo-seam-') as tmp:
    file = Path(tmp) / 'BemoNatives.java'
    file.write_text(source)
    run(os.environ.get('ELIDE', 'elide'), 'java', '--', '-jar', formatter, '--replace', file)
    expected = file.read_text()
  if check:
    if not JAVA.exists() or JAVA.read_text() != expected:
      raise RuntimeError('Generated Java is stale; run make generate-seam')
  else:
    JAVA.parent.mkdir(parents=True, exist_ok=True)
    JAVA.write_text(expected)
  return output(target)


if __name__ == '__main__':
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument('--check', action='store_true')
  parser.add_argument('--target')
  args = parser.parse_args()
  generate(args.check, args.target)
