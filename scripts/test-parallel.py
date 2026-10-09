#!/usr/bin/env python3
"""Run the workspace's test binaries concurrently, one process per binary.

`cargo test` runs its test binaries one after another; this script builds
them with the same arguments (`cargo test --no-run`), then runs whole
binaries side by side with bounded concurrency. Inside each binary the libtest
harness still runs its tests on its own threads, exactly as under
`cargo test`, so a binary's tests see the same process, working directory
(the package root), and environment either way.

    python3 scripts/test-parallel.py [--jobs N] [CARGO_TEST_ARGS...] [-- BINARY_ARGS...]

With no Cargo arguments it builds `--workspace --all-targets --locked`
(doctests are not test binaries; `cargo test --doc` runs them). Arguments
after `--` go to every binary (for example `--nocapture` or a test-name
filter). Each binary's output is written to
`$CARGO_TARGET_DIR/test-parallel/<binary>.log` (default `target/`) and
printed in full when the binary fails. The script prints every binary's
wall time, exits 1 when any binary fails, and 2 when the build fails.
Binaries start longest first, using the times the previous run recorded in
`$CARGO_TARGET_DIR/test-parallel/times.json`.
"""

import argparse
import json
import os
import re
import signal
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

DEFAULT_CARGO_ARGS = ["--workspace", "--all-targets", "--locked"]


def parse_args(argv):
    if "--" in argv:
        split = argv.index("--")
        ours, binary_args = argv[:split], argv[split + 1 :]
    else:
        ours, binary_args = argv, []
    parser = argparse.ArgumentParser(
        description="Run the workspace's test binaries concurrently.",
        usage="%(prog)s [--jobs N] [CARGO_TEST_ARGS...] [-- BINARY_ARGS...]",
    )
    parser.add_argument(
        "--jobs",
        type=int,
        default=None,
        help="test binaries to run at once (default: half the available CPUs, at least 2)",
    )
    known, cargo_args = parser.parse_known_args(ours)
    jobs = known.jobs if known.jobs is not None else max(2, (os.cpu_count() or 4) // 2)
    if jobs < 1:
        parser.error("--jobs must be at least 1")
    return jobs, cargo_args or list(DEFAULT_CARGO_ARGS), binary_args


def build(cargo_args):
    """Builds the test binaries and returns one record per binary."""
    command = [
        os.environ.get("CARGO", "cargo"),
        "test",
        "--no-run",
        "--message-format=json-render-diagnostics",
        *cargo_args,
    ]
    print("+ " + " ".join(command), flush=True)
    process = subprocess.Popen(command, stdout=subprocess.PIPE, text=True)
    binaries = []
    for line in process.stdout:
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("reason") != "compiler-artifact":
            continue
        executable = message.get("executable")
        if not executable or not message.get("profile", {}).get("test"):
            continue
        target = message["target"]
        manifest = Path(message["manifest_path"])
        package = package_name(message["package_id"], manifest)
        kind = target["kind"][0]
        binaries.append(
            {
                "name": f"{package}::{kind}::{target['name']}",
                "executable": executable,
                "package": package,
                "manifest_dir": manifest.parent,
            }
        )
    if process.wait() != 0:
        sys.exit(2)
    return binaries


def package_name(package_id, manifest):
    """The package name from a Cargo package ID (`path+file:///…/dir#name@1.0`
    or the older `name 1.0 (path+file:///…)`)."""
    fragment = re.search(r"#([^@#]+)@", package_id)
    if fragment:
        return fragment.group(1)
    if "#" not in package_id and " " in package_id:
        return package_id.split(" ", 1)[0]
    return manifest.parent.name


def target_dir():
    return Path(os.environ.get("CARGO_TARGET_DIR") or "target").resolve()


def load_times(path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return {}


class Runner:
    def __init__(self, binary_args, log_dir):
        self.binary_args = binary_args
        self.log_dir = log_dir
        self.lock = threading.Lock()
        self.children = set()
        self.stopping = False

    def run(self, binary):
        log = self.log_dir / (re.sub(r"[^A-Za-z0-9_.-]", "_", binary["name"]) + ".log")
        env = dict(os.environ)
        # What `cargo test` sets in a test binary's environment at run time.
        env["CARGO_MANIFEST_DIR"] = str(binary["manifest_dir"])
        env["CARGO_PKG_NAME"] = binary["package"]
        started = time.monotonic()
        with open(log, "wb") as output:
            with self.lock:
                if self.stopping:
                    return binary, None, 0.0, log
                child = subprocess.Popen(
                    [binary["executable"], *self.binary_args],
                    cwd=binary["manifest_dir"],
                    env=env,
                    stdin=subprocess.DEVNULL,
                    stdout=output,
                    stderr=subprocess.STDOUT,
                    start_new_session=True,
                )
                self.children.add(child)
            code = child.wait()
            with self.lock:
                self.children.discard(child)
        return binary, code, time.monotonic() - started, log

    def stop(self):
        with self.lock:
            self.stopping = True
            for child in self.children:
                try:
                    os.killpg(child.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass


def main(argv):
    jobs, cargo_args, binary_args = parse_args(argv)
    binaries = build(cargo_args)
    if not binaries:
        print("no test binaries were built", file=sys.stderr)
        return 1
    log_dir = target_dir() / "test-parallel"
    log_dir.mkdir(parents=True, exist_ok=True)
    times_path = log_dir / "times.json"
    previous = load_times(times_path)
    # Longest first, so the slowest binary never starts last.
    binaries.sort(key=lambda binary: previous.get(binary["name"], float("inf")), reverse=True)
    print(f"running {len(binaries)} test binaries, {jobs} at a time", flush=True)

    runner = Runner(binary_args, log_dir)
    results = []
    started = time.monotonic()
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        futures = [pool.submit(runner.run, binary) for binary in binaries]
        try:
            for future in as_completed(futures):
                binary, code, seconds, log = future.result()
                if code is None:
                    continue
                status = "ok" if code == 0 else f"FAILED ({code})"
                print(f"{seconds:8.1f}s  {status:<12} {binary['name']}", flush=True)
                results.append((binary, code, seconds, log))
        except KeyboardInterrupt:
            runner.stop()
            print("interrupted", file=sys.stderr)
            return 130
    wall = time.monotonic() - started

    previous.update({binary["name"]: seconds for binary, code, seconds, _ in results if code == 0})
    times_path.write_text(json.dumps(previous, indent=1, sort_keys=True) + "\n")

    failed = [result for result in results if result[1] != 0]
    for binary, code, _, log in failed:
        print(f"\n===== {binary['name']} exited {code}; output ({log}):", flush=True)
        sys.stdout.write(log.read_text(errors="replace"))
    print("\nslowest binaries:")
    for binary, code, seconds, _ in sorted(results, key=lambda result: -result[2])[:10]:
        print(f"{seconds:8.1f}s  {binary['name']}")
    total = sum(result[2] for result in results)
    print(
        f"\n{len(results) - len(failed)} passed, {len(failed)} failed; "
        f"{wall:.1f}s wall, {total:.1f}s summed over {len(results)} binaries"
    )
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
