"""Bounded extraction of the single application executable."""
from pathlib import Path
import shutil
import tarfile
from .model import PreviewError as ReleaseError


def extract_binary(archive: Path, binary: Path):
    """Extract only the executable from a bounded, regular-file native archive."""
    with tarfile.open(archive) as compressed:
        members = []
        total = 0
        for member in compressed:
            total += member.size
            if (len(members) >= 1000 or total > 512 * 1024 * 1024
                    or not (member.isfile() or member.isdir())
                    or Path(member.name).is_absolute() or '..' in Path(member.name).parts):
                raise ReleaseError('Unexpected archive type, path or expanded size.')
            members.append(member)
        if len({m.name for m in members}) != len(members):
            raise ReleaseError('Duplicate archive member.')
        binaries = [m for m in members if m.isfile() and Path(m.name).name == 'fluxcope']
        if len(binaries) != 1 or not binaries[0].mode & 0o111:
            raise ReleaseError('Expected one executable Fluxcope binary in archive.')
        with compressed.extractfile(binaries[0]) as source, binary.open('wb') as destination:
            shutil.copyfileobj(source, destination, length=1024 * 1024)
        binary.chmod(0o755)


