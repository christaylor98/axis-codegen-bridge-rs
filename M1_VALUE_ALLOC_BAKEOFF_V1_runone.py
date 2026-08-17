#!/usr/bin/env python3
"""Run one probe, report peak RSS + wall clock. Stands in for /usr/bin/time -v,
which is not installed on this box. ru_maxrss from RUSAGE_CHILDREN is the same
kernel counter `time -v` prints as "Maximum resident set size (kbytes)".

Prints the child's stdout verbatim, then a trailer:
    __rss_kb=<peak rss in KiB>
    __wall_s=<wall seconds>
    __status=<exit status>
"""
import resource
import subprocess
import sys
import time

argv = sys.argv[1:]
if not argv:
    sys.exit("usage: runone.py <cmd> [args...]")

t0 = time.monotonic()
p = subprocess.run(argv, capture_output=True, text=True)
wall = time.monotonic() - t0
ru = resource.getrusage(resource.RUSAGE_CHILDREN)

sys.stdout.write(p.stdout)
if p.stderr:
    sys.stderr.write(p.stderr)
print("__rss_kb=%d" % ru.ru_maxrss)
print("__wall_s=%.4f" % wall)
print("__status=%d" % p.returncode)
