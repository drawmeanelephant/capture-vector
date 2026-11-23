#!/usr/bin/env python3
"""Acquire and verify Oliver 1.3.0, then exercise all required frontends."""
import hashlib, json, os, platform, subprocess, sys, urllib.request
from pathlib import Path

PIN = "967bfdf428ff58b9b6e292d2f0d05184e929fb92"
ASSETS = {
    ("Linux", "x86_64"): ("oliver-linux-x86_64", "dcf914ba6d94ff02bdb10832deca1d7463f1eb4ecbb41f160792095e53c3af56"),
    ("Linux", "aarch64"): ("oliver-linux-aarch64", "25836763062c516d5980d5fa9fb953d8f31116fd2c7f5d2d6dab3206456679b9"),
    ("Darwin", "arm64"): ("oliver-macos-aarch64", "b20a6fc4eee310726f765f2a7a9c90762cb4d2aaf1ca96365c070feaac138664"),
    ("Darwin", "x86_64"): ("oliver-macos-x86_64", "c0b7096c542c878a5618384d0e9a584f74e7b6343aa1a358d2bdf39118818ab7"),
    ("Windows", "AMD64"): ("oliver-windows-x86_64.exe", "d3f995b7a0ed97399f94e80424e15da49bc22053a22fed40e66066b8c730d5c9"),
}
system, machine = platform.system(), platform.machine()
asset, expected = ASSETS[(system, machine)]
out = Path(sys.argv[1] if len(sys.argv) > 1 else ".readiness/oliver")
out.mkdir(parents=True, exist_ok=True)
binary = out / asset
url = "https://github.com/drawmeanelephant/oliver/releases/download/v1.3.0/" + asset
with urllib.request.urlopen(url, timeout=60) as response:
    payload = response.read()
actual = hashlib.sha256(payload).hexdigest()
assert actual == expected, (asset, actual, expected)
binary.write_bytes(payload)
if system != "Windows":
    binary.chmod(0o755)
def run(*args, stdin=""):
    return subprocess.run([str(binary), *args], input=stdin, text=True, capture_output=True, check=True).stdout
version = run("--version").strip()
assert version == f"oliver 1.3.0 (commit {PIN})", version
cases = [
    (("render", "--from", "markdown"), "# Readiness\n\nMarkdown **ok**.\n", "<strong>ok</strong>"),
    (("render", "--from", "textile"), "h1. Readiness\n\nTextile *ok*.\n", "<strong>ok</strong>"),
    (("render", "--from", "cooklang"), "Add @salt{1%tsp} and mix for ~{2%minutes}.\n", "salt"),
]
for args, source, needle in cases:
    rendered = run(*args, stdin=source)
    assert needle in rendered, (args, rendered)
    print(f"{args[-1]}: PASS ({len(rendered)} bytes)")
metadata = json.loads(run("meta", "--from", "markdown", "--format", "json", stdin="---\ntitle: Readiness\nstatus: published\n---\n# Body\n"))
assert metadata["title"] == "Readiness", metadata
assert set(metadata) == {"title", "description", "author", "date", "template", "palette", "render_profile"}, metadata
print(json.dumps({"binary": asset, "url": url, "sha256": actual, "version": version, "meta": "PASS"}))
