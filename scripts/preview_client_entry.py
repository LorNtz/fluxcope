"""PyInstaller entry: deliberately imports no repository release configuration."""
from preview_client.cli import entry

if __name__ == '__main__':
    entry()
