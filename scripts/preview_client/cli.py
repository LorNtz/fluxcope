"""Entry point for the bundled, repository-independent preview client."""
import argparse
import os
from pathlib import Path
import sys
from urllib.parse import urlsplit

from .lifecycle import install, launch, uninstall
from .model import PreviewError
from .remote import Downloads
from .state import State


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('install', 'launch'))
    parser.add_argument('id')
    parser.add_argument('--client-archive', type=Path)
    parser.add_argument('--port', type=int)
    parser.add_argument('--uninstall', action='store_true')
    parser.add_argument('--purge', action='store_true')
    args = parser.parse_args()
    if args.purge and not args.uninstall:
        parser.error('--purge requires --uninstall')
    if args.command == 'install' and (args.port or args.uninstall):
        parser.error('installation does not launch or remove previews')
    if args.command != 'install' and args.client_archive:
        parser.error('--client-archive is only used during installation')
    proxy = os.environ.get('all_proxy') or os.environ.get('ALL_PROXY')
    if proxy and urlsplit(proxy).scheme in ('http', 'https'):
        for scheme in ('http', 'https'):
            if f'{scheme}_proxy' not in os.environ and f'{scheme.upper()}_PROXY' not in os.environ:
                os.environ[f'{scheme}_proxy'] = proxy
    runtime = Path(sys.executable).resolve().parent
    downloads = Downloads(runtime / 'gh', runtime / 'trusted-root.jsonl')
    state = State(args.id)
    with state.lock():
        if args.uninstall:
            uninstall(state, args.purge)
        elif args.command == 'install':
            install(state, downloads, args.client_archive)
        else:
            launch(state, downloads, args.port)


def entry():
    try:
        main()
    except (PreviewError, OSError, ValueError, KeyError) as error:
        print(f'Preview: {error}', file=sys.stderr)
        sys.exit(1)
