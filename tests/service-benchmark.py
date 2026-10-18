"""Measure isolated calendar clients and the real daemon with synthetic data.

Build a release binary, then run (20 seconds of idle CPU sampling per case):
  cargo build --locked --release --manifest-path calendar-rs/Cargo.toml --target-dir /tmp/calendar-bench-target
  python3 tests/service-benchmark.py --binary /tmp/calendar-bench-target/release/omarchy-calendar
  python3 tests/service-benchmark.py --binary /tmp/calendar-bench-target/release/omarchy-calendar --items 1000 --case shared_widget --case shared_plus_fallback

Every process uses a temporary home/config/state/runtime and a network namespace
without network access. Presentation components and secret-tool are mocked;
CalendarService, BarWidget, and the Rust daemon are real. The default cache is
empty; --items seeds one synthetic account/calendar with timed events. This measures
client/cache overhead, not rendered panels, Google sync, or the full desktop shell.
RSS counts shared pages repeatedly; aggregate PSS is the better memory comparison.
"""
import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import statistics
import subprocess
import sys
import tempfile
import time

from bar_service import MOCKS


ROOT = Path(__file__).resolve().parents[1]
NAMESPACE = ["unshare", "--user", "--map-root-user", "--net"]
CASES = {
    "bare_shell": (False, False, 0),
    "daemon_only": (True, False, 0),
    "shared_widget": (True, True, 1),
    "fallback_one": (True, False, 1),
    "fallback_three": (True, False, 3),
    "shared_plus_fallback": (True, True, 1),
}

QML = """
import QtQuick
import Quickshell
import Quickshell.Io
import "plugin" as Calendar

ShellRoot {
    id: root
    property var shared: null
    property var widgets: []
    property var clients: []
    Component { id: serviceType; Calendar.CalendarService {} }
    Component { id: widgetType; Calendar.BarWidget {} }
    QtObject {
        id: providedShell
        function serviceFor(id) { return root.shared }
    }
    QtObject { id: replacementShell; function serviceFor(id) { return null } }
    QtObject { id: sharedBar; property var shell: providedShell }
    QtObject { id: replacementBar; property var shell: replacementShell }

    Component.onCompleted: {
        if (SHARED) {
            shared = serviceType.createObject(root)
            clients.push(shared)
        }
        for (var i = 0; i < WIDGETS; ++i) {
            var item = widgetType.createObject(root, {bar: INJECTED ? sharedBar : replacementBar})
            widgets.push(item)
            var client = item.calendarService
            if (clients.indexOf(client) === -1) clients.push(client)
        }
        for (var c = 0; c < clients.length; ++c)
            clients[c].loadRange("2026-10-01", "2026-11-01")
    }

    Timer {
        id: ready
        interval: 50; running: true; repeat: true
        onTriggered: {
            for (var i = 0; i < root.clients.length; ++i) {
                var client = root.clients[i]
                if (!client || !client.connected || !client.generatedAt) return
                if (client.accounts.length !== (EXPECTED_ITEMS > 0 ? 1 : 0)
                    || client.items.length !== EXPECTED_ITEMS) return
            }
            stop()
            console.log("BENCH_READY " + JSON.stringify({clients: root.clients.length, widgets: root.widgets.length}))
        }
    }
    IpcHandler {
        target: "calendarResourceBenchmark"
        function switchToShared(): void {
            for (var i = 0; i < root.widgets.length; ++i) {
                root.widgets[i].bar = sharedBar
                if (root.widgets[i].calendarService !== root.shared) {
                    console.error("BENCH_FAILED: service switch")
                    Qt.exit(1)
                }
            }
            console.log("BENCH_SWITCHED")
        }
    }
}
"""

BARE_QML = """
import QtQuick
import Quickshell
ShellRoot { Component.onCompleted: console.log("BENCH_READY " + JSON.stringify({clients: 0, widgets: 0})) }
"""


def prepare(base, case, items):
    for relative, content in MOCKS.items():
        if relative == "plugin/CalendarService.qml":
            continue
        path = base / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    for source in [ROOT / "BarWidget.qml", ROOT / "CalendarService.qml", *ROOT.glob("*.js")]:
        shutil.copyfile(source, base / "plugin" / source.name)
    shutil.copytree(ROOT / "services", base / "plugin/services")
    _, shared, widgets = CASES[case]
    source = QML.replace("SHARED", str(shared).lower()).replace("WIDGETS", str(widgets))
    source = source.replace("INJECTED", str(case == "shared_widget").lower())
    source = source.replace("EXPECTED_ITEMS", str(items))
    (base / "shell.qml").write_text(source if widgets else BARE_QML)
    for name in ("home", "config", "state", "data", "cache", "runtime", "bin"):
        (base / name).mkdir(mode=0o700)
    # The daemon's initial poll may try the synthetic account. Never use a keyring.
    secret_tool = base / "bin/secret-tool"
    secret_tool.write_text("#!/bin/sh\nexit 1\n")
    secret_tool.chmod(0o700)
    env = os.environ.copy()
    env.update(HOME=str(base / "home"), XDG_CONFIG_HOME=str(base / "config"),
               XDG_STATE_HOME=str(base / "state"), XDG_DATA_HOME=str(base / "data"),
               XDG_CACHE_HOME=str(base / "cache"),
               XDG_RUNTIME_DIR=str(base / "runtime"), XDG_CONFIG_DIRS=str(base / "config"),
               QT_QPA_PLATFORM="offscreen", QT_QPA_PLATFORMTHEME="", QT_STYLE_OVERRIDE="Fusion",
               QT_QUICK_BACKEND="software")
    env["PATH"] = str(base / "bin") + os.pathsep + env["PATH"]
    for key in ("DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "OMARCHY_PATH"):
        env.pop(key, None)
    return env


def seed_cache(base, count):
    if not count:
        return
    stamp = "2026-10-01T00:00:00Z"
    with sqlite3.connect(base / "state/omarchy/calendar/agenda.db") as database:
        database.execute("INSERT INTO accounts (account_id, google_sub, email, label, created_at, updated_at) "
                         "VALUES ('benchmark', 'benchmark', 'benchmark@example.invalid', 'Benchmark', ?, ?)",
                         (stamp, stamp))
        database.execute("INSERT INTO calendars (account_id, calendar_id, label, color) "
                         "VALUES ('benchmark', 'benchmark', 'Benchmark calendar', '#8ab4f8')")
        rows = []
        for index in range(count):
            start = datetime(2026, 10, 1, 10, tzinfo=timezone.utc) + timedelta(days=index % 28)
            rows.append((f"benchmark-{index}", f"event-{index}", f"event-{index}@benchmark.invalid",
                         start.isoformat(), f"Benchmark event {index}", start.isoformat(),
                         (start + timedelta(hours=1)).isoformat()))
        database.executemany(
            "INSERT INTO events (item_id, account_id, calendar_id, event_id, ical_uid, occurrence_start, "
            "title, start_value, end_value, all_day) VALUES (?, 'benchmark', 'benchmark', ?, ?, ?, ?, ?, ?, 0)", rows)


def descendants(pid):
    result = [pid]
    for task in Path(f"/proc/{pid}/task").iterdir():
        for child in (task / "children").read_text().split():
            result.extend(descendants(int(child)))
    return list(set(result))


def snapshot(processes):
    result = {}
    for role, process in processes.items():
        totals = dict(rss_kib=0, pss_kib=0, threads=0, fds=0, cpu_ticks=0, calendar_clients=0)
        pids = descendants(process.pid)
        for pid in pids:
            proc = Path(f"/proc/{pid}")
            memory = {line.split(":")[0]: int(line.split()[1])
                      for line in (proc / "smaps_rollup").read_text().splitlines()[1:]
                      if line.startswith(("Rss:", "Pss:"))}
            stat = (proc / "stat").read_text().rsplit(") ", 1)[1].split()
            tasks = list((proc / "task").iterdir())
            totals["rss_kib"] += memory["Rss"]
            totals["pss_kib"] += memory["Pss"]
            totals["threads"] += len(tasks)
            totals["fds"] += len(list((proc / "fd").iterdir()))
            totals["cpu_ticks"] += int(stat[11]) + int(stat[12])
            totals["calendar_clients"] += sum((task / "comm").read_text().strip() == "calendar-client" for task in tasks)
        totals["pids"] = pids
        result[role] = totals
    return result


def measure(processes, seconds):
    samples = [snapshot(processes)]
    start = time.monotonic()
    for step in range(1, 5):
        time.sleep(max(0, start + seconds * step / 4 - time.monotonic()))
        samples.append(snapshot(processes))
    elapsed = time.monotonic() - start
    per_process = {}
    for role in processes:
        latest = samples[-1][role]
        per_process[role] = {key: latest[key] for key in ("pids", "threads", "fds", "calendar_clients")}
        for key in ("rss_kib", "pss_kib"):
            values = [sample[role][key] for sample in samples]
            per_process[role][key] = round(statistics.mean(values), 1)
            per_process[role][key + "_range"] = [min(values), max(values)]
        cpu = (latest["cpu_ticks"] - samples[0][role]["cpu_ticks"]) / os.sysconf("SC_CLK_TCK")
        per_process[role]["idle_cpu_seconds"] = round(cpu, 4)
        per_process[role]["idle_cpu_percent_one_core"] = round(cpu / elapsed * 100, 4)
    keys = ("rss_kib", "pss_kib", "threads", "fds", "calendar_clients", "idle_cpu_seconds", "idle_cpu_percent_one_core")
    return {"sample_seconds": round(elapsed, 3), "processes": per_process,
            "total": {key: round(sum(part[key] for part in per_process.values()), 4) for key in keys}}


def wait_for(log, marker, process, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        output = log.read_text()
        if process.poll() is not None or "BENCH_FAILED" in output or "ERROR" in output:
            raise RuntimeError(output)
        if marker in output:
            return output
        time.sleep(0.05)
    raise RuntimeError(f"Timed out waiting for {marker}:\n{log.read_text()}")


def exercise(case, args, repetition):
    with tempfile.TemporaryDirectory(prefix="calendar-service-bench-") as directory:
        base = Path(directory)
        env = prepare(base, case, args.items)
        processes, logs, handles = {}, {}, []
        try:
            commands = {}
            if CASES[case][0]:
                commands["daemon"] = [str(args.binary), "service", "run"]
            commands["shell"] = ["quickshell", "--no-color", "--path", str(base / "shell.qml")]
            for role, command in commands.items():
                logs[role] = base / (role + ".log")
                handle = logs[role].open("w")
                handles.append(handle)
                processes[role] = subprocess.Popen(NAMESPACE + command, env=env, stdout=handle, stderr=subprocess.STDOUT)
                if role == "daemon":
                    deadline = time.monotonic() + 10
                    while not (base / "runtime/omarchy-calendar.sock").exists():
                        if processes[role].poll() is not None or time.monotonic() > deadline:
                            raise RuntimeError(logs[role].read_text())
                        time.sleep(0.05)
                    seed_cache(base, args.items)
            wait_for(logs["shell"], "BENCH_READY", processes["shell"])
            time.sleep(args.warmup)
            result = [{"case": case, "repetition": repetition, **measure(processes, args.seconds)}]
            expected = (1 if CASES[case][2] else 0) + (1 if case == "shared_plus_fallback" else 0)
            if result[0]["total"]["calendar_clients"] != expected:
                raise RuntimeError(f"{case}: expected {expected} connected clients: {result[0]}")
            if case == "shared_plus_fallback":
                subprocess.run(NAMESPACE + ["quickshell", "--path", str(base / "shell.qml"), "ipc", "call",
                               "calendarResourceBenchmark", "switchToShared"], env=env,
                               capture_output=True, text=True, check=True, timeout=10)
                wait_for(logs["shell"], "BENCH_SWITCHED", processes["shell"])
                time.sleep(args.warmup)
                result.append({"case": "switch_back", "repetition": repetition, **measure(processes, args.seconds)})
            for role, process in processes.items():
                output = logs[role].read_text()
                if process.poll() is not None or "BENCH_FAILED" in output or "ERROR" in output:
                    raise RuntimeError(logs[role].read_text())
            return result
        finally:
            for process in reversed(list(processes.values())):
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            for handle in handles:
                handle.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=lambda value: Path(value).resolve())
    parser.add_argument("--seconds", type=float, default=20)
    parser.add_argument("--warmup", type=float, default=3)
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--items", type=int, default=0)
    parser.add_argument("--case", choices=CASES, action="append")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.seconds <= 0 or args.warmup < 0 or args.repeat < 1 or args.items < 0:
        parser.error("seconds/repeat must be positive; warmup/items must be nonnegative")
    subprocess.run(NAMESPACE + ["true"], check=True)
    metadata = {
        "fixture": "synthetic cache; real client/daemon; mocked presentation/secret-tool; network disabled",
        "items_per_client": args.items,
        "fixture_range": ["2026-10-01", "2026-11-01"],
        "warmup_seconds": args.warmup,
        "cpu_resolution_seconds": 1 / os.sysconf("SC_CLK_TCK"),
        "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
        "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "source_hashes": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
                          for name in ("BarWidget.qml", "CalendarService.qml", "Model.js", "I18n.js",
                                       "services/FallbackCalendarService.qml", "services/qmldir")},
        "quickshell": subprocess.check_output(["quickshell", "--version"], text=True).strip(),
    }
    rows = []
    for repetition in range(1, args.repeat + 1):
        for case in args.case or CASES:
            print(f"Measuring {case}, repetition {repetition}", file=sys.stderr, flush=True)
            rows.extend(exercise(case, args, repetition))
    for name, digest in metadata["source_hashes"].items():
        if hashlib.sha256((ROOT / name).read_bytes()).hexdigest() != digest:
            raise RuntimeError(f"{name} changed during the measurement; rerun with stable sources")
    result = json.dumps({"metadata": metadata, "results": rows}, indent=2)
    if args.output:
        args.output.write_text(result + "\n")
    print(result)


if __name__ == "__main__":
    main()
