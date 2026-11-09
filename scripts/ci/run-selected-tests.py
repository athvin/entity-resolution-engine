#!/usr/bin/env python3
"""Run complete package suites without shell-interpolating selector output."""
import json
import os
import re
import subprocess

packages = json.loads(os.environ["SELECTED_PACKAGES"])
if not packages or not all(isinstance(p, str) and re.fullmatch(r"[a-z][a-z0-9_-]*", p) for p in packages):
    raise ValueError("expected a nonempty package list")
subprocess.run(["cargo", "test", "--locked", *[arg for p in packages for arg in ("-p", p)]], check=True)
