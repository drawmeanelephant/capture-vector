#!/usr/bin/env python3
"""Prove unsigned test-package layouts using a disposable probe executable."""
import os, platform, stat, subprocess, tarfile, tempfile, zipfile
from pathlib import Path

system = platform.system()
with tempfile.TemporaryDirectory(prefix="cv-package-probe-") as raw:
    root = Path(raw)
    stage = root / "stage"
    if system == "Darwin":
        exe = stage / "CaptureVectorProbe.app" / "Contents" / "MacOS" / "CaptureVectorProbe"
        exe.parent.mkdir(parents=True)
        exe.write_text("#!/bin/sh\necho capture-vector-package-probe\n")
        exe.chmod(0o755)
        (exe.parents[1] / "Info.plist").write_text('<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleExecutable</key><string>CaptureVectorProbe</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>')
        archive = root / "probe-macos.zip"
        with zipfile.ZipFile(archive, "w") as z:
            for p in stage.rglob("*"):
                if p.is_file():
                    info = zipfile.ZipInfo(str(p.relative_to(stage)))
                    info.external_attr = (p.stat().st_mode & 0xFFFF) << 16
                    z.writestr(info, p.read_bytes())
        with zipfile.ZipFile(archive) as z:
            assert "CaptureVectorProbe.app/Contents/MacOS/CaptureVectorProbe" in z.namelist()
            assert z.getinfo("CaptureVectorProbe.app/Contents/MacOS/CaptureVectorProbe").external_attr & 0o1110000000000000
        print(f"macOS zipped .app: PASS ({archive.stat().st_size} bytes, executable mode retained)")
    elif system == "Windows":
        stage.mkdir()
        exe = stage / "CaptureVectorProbe.exe"
        exe.write_bytes(b"MZ-readiness-probe")
        (stage / "README.txt").write_text("unsigned portable validation probe")
        archive = root / "probe-windows.zip"
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as z:
            for p in stage.iterdir(): z.write(p, p.name)
        with zipfile.ZipFile(archive) as z:
            assert set(z.namelist()) == {"CaptureVectorProbe.exe", "README.txt"}
            assert z.read("CaptureVectorProbe.exe").startswith(b"MZ")
        print(f"Windows portable zip: PASS ({archive.stat().st_size} bytes)")
    elif system == "Linux":
        stage.mkdir()
        exe = stage / "capture-vector-probe"
        exe.write_text("#!/bin/sh\necho capture-vector-package-probe\n")
        exe.chmod(0o755)
        archive = root / "probe-linux.tar.gz"
        with tarfile.open(archive, "w:gz") as t: t.add(exe, arcname=exe.name)
        with tarfile.open(archive, "r:gz") as t:
            member = t.getmember(exe.name)
            assert member.mode & stat.S_IXUSR
            t.extract(member, root / "unpacked", filter="data")
        result = subprocess.run([str(root / "unpacked" / exe.name)], capture_output=True, text=True, check=True)
        assert result.stdout.strip() == "capture-vector-package-probe"
        print(f"Linux tar.gz: PASS ({archive.stat().st_size} bytes, executable ran)")
    else:
        raise SystemExit(f"unsupported package surface: {system}")
