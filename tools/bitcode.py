#!/usr/bin/env python3
"""Build and verify a separate LLVM ThinLTO static archive for Bemo consumers."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess

import seam

ROOT = seam.ROOT
DIRECTORY = ROOT / 'build/bitcode'
ARCHIVE = DIRECTORY / 'libbemo_ffi_thinlto.a'


def inventory(data):
  if not data.startswith(b'!<arch>\n'):
    raise RuntimeError('Expected a regular static archive')
  groups = {'bitcode': [], 'native': [], 'metadata': []}
  position, strings = 8, b''
  while position < len(data):
    header = data[position:position + 60]
    if len(header) != 60 or header[58:60] != b'`\n':
      raise RuntimeError('Truncated or invalid archive member header')
    try:
      size = int(header[48:58])
    except ValueError as error:
      raise RuntimeError('Invalid archive member size') from error
    start = position + 60
    if size < 0 or start + size > len(data):
      raise RuntimeError('Truncated archive member payload')
    payload = data[start:start + size]
    name = header[:16].decode().strip()
    position = start + size + (size % 2)
    if position > len(data):
      raise RuntimeError('Truncated archive member padding')
    if name == '//':
      strings = payload
      continue
    if name in ('/', '/SYM64/'):
      continue
    if name.startswith('#1/'):
      length = int(name[3:])
      if length > len(payload):
        raise RuntimeError('Truncated archive member name')
      name = payload[:length].rstrip(b'\0').decode()
      payload = payload[length:]
    elif name.startswith('/') and name[1:].isdigit():
      offset = int(name[1:])
      if offset >= len(strings):
        raise RuntimeError('Invalid archive name offset')
      name = strings[offset:].split(b'\n', 1)[0].rstrip(b'/').decode()
    else:
      name = name.rstrip('/')
    if name.startswith('__.SYMDEF'):
      continue
    if payload.startswith((b'BC\xc0\xde', b'\xde\xc0\x17\x0b')):
      groups['bitcode'].append(name)
    elif name.endswith('.rmeta'):
      groups['metadata'].append(name)
    elif payload.startswith((b'\x7fELF', b'\xcf\xfa\xed\xfe', b'\xce\xfa\xed\xfe', b'\xfe\xed\xfa\xcf')):
      groups['native'].append(name)
    else:
      raise RuntimeError(f'Unsupported archive member payload: {name}')
  if not groups['bitcode']:
    raise RuntimeError('Archive contains no LLVM bitcode')
  return groups


def tool(name):
  if os.environ.get('LLVM_BIN'):
    path = Path(os.environ['LLVM_BIN']) / name
    if not path.is_file():
      path = shutil.which(name)
  else:
    path = shutil.which(name)
  if not path or not Path(path).is_file():
    raise RuntimeError(f'{name} is required; configure LLVM_BIN to a compatible LLVM toolchain')
  return str(path)


def toolchain():
  rust = subprocess.check_output(['rustc', '-vV'], cwd=ROOT, text=True)
  llvm = re.search(r'^LLVM version: (\S+)', rust, re.M)[1]
  clang = subprocess.check_output([tool('clang'), '--version'], text=True)
  if not re.search(r'clang version ' + re.escape(llvm) + r'\b', clang):
    raise RuntimeError(f'Clang must match rustc LLVM {llvm}; configure LLVM_BIN')
  linker_name = 'ld64.lld' if seam.target_triple().endswith('apple-darwin') else 'ld.lld'
  linker = subprocess.check_output([tool(linker_name), '--version'], text=True)
  if llvm not in linker:
    raise RuntimeError(f'LLD must match rustc LLVM {llvm}; configure LLVM_BIN')
  config = subprocess.check_output([tool('llvm-config'), '--version'], text=True).strip()
  if config != llvm:
    raise RuntimeError(f'LLVM helper libraries must match rustc LLVM {llvm}')
  return {'rustc': rust.strip(), 'clang': clang.splitlines()[0], 'linker': linker.strip(), 'llvm': llvm}


def apply(archive, contracts):
  config = tool('llvm-config')
  llvm_dir = subprocess.check_output([config, '--cmakedir'], text=True).strip()
  checkout = Path(os.environ.get('SVMGEN_HOME', ROOT / 'build/tools/svmgen')).resolve()
  helper_dir = ROOT / 'build/tools/svmgen-llvm'
  seam.run('cmake', '-S', checkout / 'llvm', '-B', helper_dir,
           f'-DLLVM_DIR={llvm_dir}', '-DCMAKE_BUILD_TYPE=Release',
           f'-DCMAKE_C_COMPILER={tool("clang")}', f'-DCMAKE_CXX_COMPILER={tool("clang++")}')
  seam.run('cmake', '--build', helper_dir, '--parallel', '4')
  # The helper owns all ABI/contract validation. Invoke it directly from the
  # build driver so JVM subprocess configuration cannot affect native tooling.
  seam.run(helper_dir / 'svmgen-llvm', '--contracts', contracts, '--input', archive,
           '--output', ARCHIVE)


def build():
  versions = toolchain()
  DIRECTORY.mkdir(parents=True, exist_ok=True)
  metadata = seam.generate(check=True)
  flags = '-Clinker-plugin-lto'
  env = dict(os.environ)
  env.pop('CARGO_ENCODED_RUSTFLAGS', None)
  env['RUSTFLAGS'] = flags
  # Native dependencies can inherit linker-plugin LTO through cc. Archive
  # them with LLVM tools so Apple ar cannot omit bitcode members.
  env['CC'] = tool('clang')
  env['CXX'] = tool('clang++')
  env['AR'] = tool('llvm-ar')
  env['RANLIB'] = tool('llvm-ranlib')
  if seam.target_triple().endswith('apple-darwin'):
    env.setdefault('MACOSX_DEPLOYMENT_TARGET', '15.0')
    # Use the cc builder, which honors AR. CMake's Apple default archiver
    # silently omits bcm bitcode as an unknown architecture.
    env['AWS_LC_SYS_CMAKE_BUILDER'] = '0'
  seam.run('cargo', 'rustc', '-p', 'bemo-ffi', '--lib', '--locked', '--profile', 'bitcode',
           '--crate-type', 'staticlib', '--target', seam.target_triple(),
           '--target-dir', DIRECTORY / 'cargo', env=env)
  original = DIRECTORY / 'cargo' / seam.target_triple() / 'bitcode/libbemo_ffi.a'
  apply(original, metadata / 'seam.ll')
  members = inventory(ARCHIVE.read_bytes())
  pins = json.loads((ROOT / 'tools/versions.json').read_text())['svmgen']
  manifest = {'schema': 1, 'target': seam.target_triple(), 'archive': ARCHIVE.name,
              'sha256': hashlib.sha256(ARCHIVE.read_bytes()).hexdigest(), 'toolchain': versions,
              'rustflags': flags, 'profile': 'bitcode', 'generator': pins,
              'seamFingerprint': (metadata / 'seam.abi').read_text().strip(),
              'members': members, 'nativeDependencyPolicy': 'C dependencies may inherit ThinLTO; assembly and prebuilt std members stay native',
              'nativeTools': {key: Path(env[key]).name for key in ('CC', 'CXX', 'AR', 'RANLIB')}}
  (DIRECTORY / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
  print(f'Bitcode archive: {ARCHIVE}; {len(members["bitcode"])} IR, {len(members["native"])} native members')
  return ARCHIVE


def verify(archive, directory):
  """Run the ownership contract and inspect actual saved ThinLTO output."""
  toolchain()
  inventory(Path(archive).read_bytes())
  directory = Path(directory).resolve()
  directory.mkdir(parents=True, exist_ok=True)
  binary = directory / 'thinlto-contract'
  saved = '-Wl,-save-temps' if seam.target_triple().endswith('apple-darwin') else '-Wl,--save-temps'
  for saved_file in Path(archive).parent.glob(Path(archive).name + '(*.bc'):
    saved_file.unlink()
  for saved_file in directory.glob('*.bc'):
    saved_file.unlink()
  libraries = ['-ldl', '-lpthread', '-lm'] if seam.target_triple().endswith('linux-gnu') else []
  headers = Path(archive).parent if (Path(archive).parent / 'bemo.h').exists() else ROOT / 'include'
  seam.run(tool('clang'), '-O2', '-flto=thin', '-fuse-ld=lld', saved,
           '-std=c11', '-Wall', '-Wextra', '-Werror', '-I', headers,
           ROOT / 'tests/static.c', archive, *libraries, '-o', binary)
  seam.run(binary)
  # LLD places per-member intermediate files alongside the input archive.
  files = list(directory.rglob('*.opt.bc'))
  files += list(Path(archive).parent.glob(Path(archive).name + '(*.opt.bc'))
  found = False
  for file in files:
    text = subprocess.check_output([tool('llvm-dis'), str(file), '-o', '-'], text=True)
    if re.search(r'^define .*@elide_transport_owner_new\(', text, re.M):
      found = True
      break
  if not found:
    raise RuntimeError('Saved ThinLTO output contains no Bemo definition; archive did not participate')
  print('ThinLTO ownership consumer passed; Bemo definition present in optimized linker IR')


if __name__ == '__main__':
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument('task', choices=('build', 'test'))
  args = parser.parse_args()
  if args.task == 'build':
    build()
  else:
    verify(build(), DIRECTORY / 'verify')
