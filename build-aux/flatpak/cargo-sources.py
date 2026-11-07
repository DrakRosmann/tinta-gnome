#!/usr/bin/env python3
"""The crates in Cargo.lock as flatpak-builder sources, for a build without network.

    build-aux/flatpak/cargo-sources.py > build-aux/flatpak/cargo-sources.json

Run it again whenever Cargo.lock changes. Each crate is downloaded from crates.io and checked
against the lock file's checksum; a cargo config points cargo at them instead of the network.
"""

import json
import sys
import tomllib
from pathlib import Path

root = Path(__file__).resolve().parents[2]
lock = tomllib.loads((root / "Cargo.lock").read_text())

sources = []
for pkg in lock["package"]:
    source = pkg.get("source", "")
    if not source.startswith("registry+"):
        # The workspace's own crates, and nothing else: there are no git dependencies.
        if source:
            sys.exit(f"{pkg['name']}: only crates.io dependencies are handled, not {source}")
        continue
    name, version, checksum = pkg["name"], pkg["version"], pkg["checksum"]
    dest = f"cargo/vendor/{name}-{version}"
    sources.append({
        "type": "archive",
        "archive-type": "tar-gzip",
        "url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
        "sha256": checksum,
        "dest": dest,
    })
    sources.append({
        "type": "inline",
        "contents": json.dumps({"package": checksum, "files": {}}),
        "dest": dest,
        "dest-filename": ".cargo-checksum.json",
    })

sources.append({
    "type": "inline",
    "contents": '[source.vendored-sources]\ndirectory = "cargo/vendor"\n\n[source.crates-io]\nreplace-with = "vendored-sources"\n',
    "dest": "cargo",
    "dest-filename": "config.toml",
})

json.dump(sources, sys.stdout, indent=2)
print()
