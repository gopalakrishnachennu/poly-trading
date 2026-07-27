#!/usr/bin/env python3
"""Start a command in its own session so it survives the launching shell.

macOS has no setsid(1), and Cursor's shell terminates the whole process group
when a command returns, which silently killed supervised workers. Double-fork
with os.setsid() so the child is reparented to init and keeps running.

Usage: scripts/detach.py LOGFILE COMMAND [ARGS...]
"""

import os
import sys

if len(sys.argv) < 3:
    sys.exit(__doc__)

log_path, argv = sys.argv[1], sys.argv[2:]

if os.fork() > 0:
    os._exit(0)
os.setsid()
if os.fork() > 0:
    os._exit(0)

log = os.open(log_path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
devnull = os.open(os.devnull, os.O_RDONLY)
os.dup2(devnull, 0)
os.dup2(log, 1)
os.dup2(log, 2)
os.execvp(argv[0], argv)
