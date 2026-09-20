"""Private state, serialized updates, and strict paths for preview installations."""
from contextlib import contextmanager
import fcntl
import os
from pathlib import Path
import platform
import tempfile

from .model import PreviewError, preview_id


def host_target():
    machine = {'arm64': 'aarch64', 'aarch64': 'aarch64', 'x86_64': 'x86_64'}.get(platform.machine())
    system = platform.system()
    if not machine or system not in ('Darwin', 'Linux'):
        raise PreviewError('Previews support native macOS/Linux ARM64 and x86-64.')
    if system == 'Darwin' and int(platform.mac_ver()[0].split('.')[0]) < 15:
        raise PreviewError('macOS previews require macOS 15 or newer.')
    if system == 'Linux':
        libc, version = platform.libc_ver()
        if libc != 'glibc' or tuple(map(int, version.split('.')[:2])) < (2, 28):
            raise PreviewError('Linux previews require glibc 2.28 or newer; musl is unsupported.')
    return machine + ('-apple-darwin' if system == 'Darwin' else '-unknown-linux-gnu')


def checked(path, *, directory=False):
    """Reject symlinks at every component, even when the final path is absent."""
    if not path.is_absolute():
        raise PreviewError('Preview paths must be absolute.')
    for component in (*reversed(path.parents), path):
        if component.is_symlink():
            raise PreviewError(f'Preview path traverses a symlink: {component}')
    if path.exists():
        if path.stat().st_uid != os.getuid() or (directory and not path.is_dir()) or (not directory and not path.is_file()):
            raise PreviewError(f'Preview path has an unexpected owner or type: {path}')
    return path


def private_directory(path):
    checked(path, directory=True)
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.chmod(0o700)
    return path


def atomic_text(path, contents, mode=0o600):
    checked(path)
    with tempfile.NamedTemporaryFile(mode='w', dir=path.parent, delete=False) as stream:
        temporary = Path(stream.name)
        try:
            stream.write(contents)
            stream.flush()
            os.fsync(stream.fileno())
            temporary.chmod(mode)
            os.replace(temporary, path)
        finally:
            temporary.unlink(missing_ok=True)


class State:
    def __init__(self, identifier, target=None, home=None):
        self.id = preview_id(identifier)
        self.target = target or host_target()
        self.home = home or Path.home()
        self.base = self.home / '.cache/fluxcope/previews'
        self.root = self.base / str(self.id) / self.target
        self.launcher = self.home / f'.local/bin/fluxcope-preview-{self.id}'

    @contextmanager
    def lock(self):
        for path in (self.base.parent, self.base, self.root.parent, self.root):
            private_directory(path)
        lock = checked(self.base / f'.{self.id}.lock')
        descriptor = os.open(lock, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        try:
            try:
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as error:
                raise PreviewError('This preview is already running or being changed; close it and retry.') from error
            yield
        finally:
            os.close(descriptor)
