"""Module entry point for `python3 -m appbox`."""

import sys

from appbox.cli import main

if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
