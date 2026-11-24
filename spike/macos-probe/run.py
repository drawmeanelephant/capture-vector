#!/usr/bin/env python3
"""Native macOS probes over the real cv-app, using only a throwaway corpus."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time


ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "target/macos-probe"
NATIVE = OUT / "native"


def run(*args):
    return subprocess.check_output(list(map(str, args)), text=True, timeout=15).strip()


def native(*args):
    return run(NATIVE, *args)


def walk(node):
    yield node
    for child in node.get("children", []):
        yield from walk(child)


def wait_for(read, predicate, description):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        value = read()
        if predicate(value):
            return value
        time.sleep(0.1)
    raise AssertionError(f"Timed out: {description}; last value = {value!r}")


def stop(process, graceful=False):
    if process.poll() is not None:
        return
    if graceful:
        try:
            native("quit", process.pid)
            process.wait(timeout=10)
            return
        except subprocess.SubprocessError as error:
            print(f"WARN native quit failed: {error}; terminating the app", flush=True)
    process.terminate()
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=10)


class Probe:
    def __init__(self, process, project, output):
        self.process = process
        self.pid = process.pid
        self.project = project
        self.output = output
        self.input_target = None

    def windows(self):
        assert self.process.poll() is None, f"App exited: see {self.output / 'app.log'}"
        return json.loads(native("inspect", self.pid))

    def element(self, identifier):
        return next(
            (e for w in self.windows() for e in walk(w) if e.get("AXIdentifier") == identifier),
            None,
        )

    def body(self):
        e = self.element("editor-body")
        return e.get("AXValue") if e else None

    def expect_body(self, expected):
        wait_for(self.body, lambda value: value == expected, f"editor body = {expected!r}")

    def activate(self):
        run(
            "osascript", "-e",
            'tell application "System Events" to set frontmost of '
            f"(first application process whose unix id is {self.pid}) to true",
        )

    def key(self, identifier, code, *modifiers):
        if self.input_target != identifier:
            self.activate()
            wait_for(lambda: self.element(identifier), bool, f"native input target {identifier}")
            native("focus", self.pid, identifier)
            time.sleep(0.1)
            self.input_target = identifier
        native("key", self.pid, code, *modifiers)

    def replace(self, value):
        native("replace", self.pid, "editor-body", value)
        self.expect_body(value)
        self.key("editor-body", 124, "cmd")  # End of the single-line baseline.

    def close(self, title):
        windows = self.windows()
        index = next(i for i, w in enumerate(windows) if w.get("AXTitle") == title)
        native("close", self.pid, index)
        self.input_target = None

    def snapshot(self, name, title="Capture Vector — editor"):
        windows = json.loads(native("windows", self.pid))
        window = next(w for w in windows if w.get("kCGWindowName") == title)
        run("screencapture", "-x", "-l", window["kCGWindowNumber"], self.output / f"{name}.png")
        (self.output / f"{name}.ax.json").write_text(json.dumps(self.windows(), indent=2) + "\n")

    def check(self, name):
        print(f"PASS {name}", flush=True)

    def editor(self):
        wait_for(self.body, lambda text: text is not None, "native editor opens")
        self.snapshot("initial")
        self.check("primary + editor are real native windows")

        native("source", "com.apple.keylayout.US")
        self.replace("caf")
        self.key("editor-body", 14, "alt")
        self.expect_body("caf")
        self.snapshot("us-preedit")
        self.key("editor-body", 14)
        self.expect_body("café")
        self.snapshot("us-committed")
        self.check("native US dead-key preedit stays uncommitted; commit is café")

        self.replace("caf")
        self.key("editor-body", 14, "alt")
        self.key("editor-body", 53)  # US dead-key Escape commits the literal accent.
        self.expect_body("caf´")
        self.key("editor-body", 11)  # b
        self.expect_body("caf´b")
        self.check("US dead-key Escape matches Cocoa; typing continues after composition")

        self.close("Capture Vector — editor")
        wait_for(lambda: self.element("editor-close-prompt"), bool, "dirty title-bar close reopens")
        self.expect_body("caf´b")
        self.snapshot("dirty-close")
        native("press", self.pid, "editor-close-cancel")
        wait_for(lambda: self.element("editor-close-prompt"), lambda e: e is None, "keep editing")
        self.expect_body("caf´b")
        self.check("dirty native close reopens with edits; keep editing preserves them")

        self.close("Capture Vector — editor")
        wait_for(lambda: self.element("editor-close-save"), bool, "save-and-close prompt")
        native("press", self.pid, "editor-close-save")
        wait_for(self.body, lambda text: text is None, "save closes the native editor")
        saved = (self.project / "content/index.md").read_text()
        assert saved.endswith("---\ncaf´b"), repr(saved)
        self.check("save-and-close persists exactly the edited bytes and closes")

    def companions(self):
        selected = "row-guides/publishing.md"
        native("press", self.pid, selected)
        self.key(selected, 36)
        wait_for(self.windows, lambda w: any(x.get("AXTitle") == "Capture Vector — companion" for x in w),
                 "reading companion opens")
        native("press", self.pid, "row-guides/getting-started.md")
        companion = next(w for w in self.windows() if w.get("AXTitle") == "Capture Vector — companion")
        texts = [e["AXValue"] for e in walk(companion) if e.get("AXRole") == "AXStaticText"]
        assert texts[0] == "Publishing", texts
        self.snapshot("pinned-companion", "Capture Vector — companion")
        self.check("moving primary selection does not retarget its native companion")

        native("press", self.pid, selected)
        self.key(selected, 36)
        time.sleep(0.2)
        assert sum(w.get("AXTitle") == "Capture Vector — companion" for w in self.windows()) == 1
        self.check("reopening the same noun creates no duplicate native companion")
        self.close("Capture Vector — companion")
        wait_for(self.windows, lambda w: not any(x.get("AXTitle") == "Capture Vector — companion" for x in w),
                 "companion closes")
        self.key(selected, 36)
        wait_for(self.windows, lambda w: any(x.get("AXTitle") == "Capture Vector — companion" for x in w),
                 "same noun reopens after a native close")
        self.check("closed companion can be reopened without a stale native handle")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.parse_args()
    if sys.platform != "darwin":
        parser.error("This probe requires a real macOS desktop.")
    OUT.mkdir(parents=True, exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix="run-", dir=OUT))
    with (output / "build.log").open("w") as log:
        subprocess.run(["cargo", "build", "-p", "cv-app", "--locked"], cwd=ROOT,
                       stdout=log, stderr=subprocess.STDOUT, check=True, timeout=180)
    subprocess.run(["swiftc", ROOT / "spike/macos-probe/native.swift", "-o", NATIVE],
                   check=True, timeout=60)
    subprocess.run(["swiftc", ROOT / "spike/macos-probe/reference.swift", "-o", OUT / "reference"],
                   check=True, timeout=60)
    previous_source = native("source")
    project = output / "corpus"
    shutil.copytree(ROOT / "fixtures/corpus-src/happy", project)
    env = os.environ.copy()
    env.update(CV_CONFIG_DIR=str(output / "config"), CV_EDIT="content/index.md")
    with (output / "app.log").open("w") as log:
        reference = subprocess.Popen([OUT / "reference"], stdout=log, stderr=subprocess.STDOUT)
        try:
            native("source", "com.apple.keylayout.US")
            control = Probe(reference, project, output)
            control.expect_body("caf")
            control.key("editor-body", 14, "alt")
            control.key("editor-body", 53)
            control.expect_body("caf´")
            control.snapshot("cocoa-escape", "Cocoa IME control")
            control.check("independent Cocoa control: US dead-key Escape leaves caf´")
        finally:
            try:
                native("source", previous_source)
            finally:
                stop(reference)
        process = subprocess.Popen(
            [ROOT / "target/debug/cv-app", "--project", project, "--dirs-root", output / "rune"],
            cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT,
        )
        try:
            probe = Probe(process, project, output)
            probe.editor()
            probe.companions()
        except Exception:
            try:
                probe.snapshot("failure")
            except (subprocess.SubprocessError, StopIteration):
                pass
            print(f"Failed-run artifacts: {output.relative_to(ROOT)}", flush=True)
            raise
        finally:
            try:
                native("source", previous_source)
            finally:
                stop(process, graceful=True)
    print(f"Artifacts: {output.relative_to(ROOT)}", flush=True)


if __name__ == "__main__":
    main()
