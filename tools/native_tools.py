"""Keep Elide's cc language entrypoint out of native compiler discovery."""
import os
from pathlib import Path
import shutil
import subprocess


def environment(env, directory):
  env = dict(env)
  cc = shutil.which('cc', path=env.get('PATH', ''))
  if not cc:
    return env
  resolved = Path(cc).resolve()
  if resolved.name == 'mise':
    # Mise shims all resolve to its executable; inspect the selected cc instead.
    result = subprocess.run([resolved, 'which', 'cc'], env=env, text=True,
                            capture_output=True, check=False)
    if result.returncode:
      return env
    resolved = Path(result.stdout.strip())
  with resolved.open('rb') as source:
    alias = b'Dist entrypoint for alias: cc' in source.read(512)
  if not alias:
    return env
  compiler = None
  for entry in env.get('PATH', '').split(os.pathsep):
    candidate = shutil.which('clang', path=entry)
    if not candidate:
      continue
    if Path(candidate).resolve().name == 'mise':
      result = subprocess.run([Path(candidate).resolve(), 'which', 'clang'], env=env,
                              text=True, capture_output=True, check=False)
      if result.returncode:
        continue
      candidate = result.stdout.strip()
    compiler = candidate
    break
  if not compiler:
    raise RuntimeError('Elide cc shadows the native compiler; install Clang or select a native toolchain')
  # Only cc is shadowed. Keep OpenSSL, Java, Python, Cargo and all user compiler
  # flags in their existing order; a whole system-bin PATH prepend changes them.
  directory = Path(directory).absolute()
  directory.mkdir(parents=True, exist_ok=True)
  wrapper = directory / 'cc'
  if not wrapper.is_symlink() or os.readlink(wrapper) != compiler:
    wrapper.unlink(missing_ok=True)
    wrapper.symlink_to(compiler)
  env['PATH'] = str(directory) + os.pathsep + env.get('PATH', '')
  return env


def configure():
  directory = Path(__file__).resolve().parents[1] / 'build/tools/native-cc'
  os.environ.update(environment(os.environ, directory))
