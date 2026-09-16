#!/usr/bin/env python3
"""Rewrites the worked example in docs/SPEC.md §7 from the reference writer's actual output."""
import pathlib, re, subprocess, sys

root = pathlib.Path(__file__).resolve().parent.parent
run = subprocess.run(["cargo", "run", "-q", "-j", "16", "--example", "dump_minimal"],
                     capture_output=True, text=True, cwd=root)
if run.returncode != 0:
    sys.exit(run.stderr.strip() or "dump_minimal failed")
dump = run.stdout.rstrip()
meta = run.stderr.strip()

ann = {
    0x00: "magic, v1.0, feature_flags=0",
    0x20: "manifest_root (BLAKE3 of the manifest bytes)",
    0x40: "sect_count  flags=0 (uncompressed)  name_idx  desc_idx",
}
out = []
for line in dump.splitlines():
    off = int(line[:8], 16)
    a = ann.get(off, "")
    out.append((line.rstrip() + ("   " + a if a else "")).rstrip())

p = root / "docs" / "SPEC.md"
s = p.read_text()
s = re.sub(r"```\n00000000  8f 53 4b 4c.*?\n```", "```\n" + "\n".join(out) + "\n```", s, flags=re.S)
p.write_text(s)
print(f"refreshed: {meta}")
