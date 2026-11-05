"""Keep Elide's cc language entrypoint out of native compiler discovery."""
import os
from pathlib import Path
import shutil
import subprocess


def compiler(path, env):
  resolved = Path(path).resolve()
  if resolved.name == 'mise':
    # Mise shims all resolve to its executable; inspect the selected cc instead.
    result = subprocess.run([resolved, 'which', 'cc'], env=env, text=True,
                            capture_output=True, check=False)
    if result.returncode:
      return None
    resolved = Path(result.stdout.strip()).resolve()
  with resolved.open('rb') as source:
    if b'Dist entrypoint for alias: cc' in source.read(512):
      return None
  return resolved


def environment(env, directory):
  env = dict(env)
  cc = shutil.which('cc', path=env.get('PATH', ''))
  if not cc or compiler(cc, env) is not None:
    return env
  selected = None
  for entry in env.get('PATH', '').split(os.pathsep):
    candidate = shutil.which('cc', path=entry)
    if candidate:
      selected = compiler(candidate, env)
      if selected is not None:
        break
  if selected is None:
    raise RuntimeError('Elide cc shadows the native compiler; select a native toolchain')
  # Only cc is shadowed. Preserve the platform compiler (Apple Clang on macOS),
  # OpenSSL, Java, Python, Cargo, and all explicit compiler flags and overrides.
  directory = Path(directory).absolute()
  directory.mkdir(parents=True, exist_ok=True)
  wrapper = directory / 'cc'
  if not wrapper.is_symlink() or os.readlink(wrapper) != str(selected):
    wrapper.unlink(missing_ok=True)
    wrapper.symlink_to(selected)
  env['PATH'] = str(directory) + os.pathsep + env.get('PATH', '')
  return env


def configure():
  directory = Path(__file__).resolve().parents[1] / 'build/tools/native-cc'
  os.environ.update(environment(os.environ, directory))
