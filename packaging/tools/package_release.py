#!/usr/bin/env python3
"""Build deterministic, dependency-free Neutrasearch portable archives."""
from package_release import *  # noqa: F403
from package_release import main

if __name__ == "__main__":
    raise SystemExit(main())
