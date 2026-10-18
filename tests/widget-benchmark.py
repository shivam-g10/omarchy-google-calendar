#!/usr/bin/env python3
"""Measure production widget construction, open latency, and idle resources.

Uses the panel benchmark's offscreen platform wrapper and the service benchmark's
Linux process sampler. Calendar state is synthetic; no daemon is started. The
normal Omarchy controls and every calendar widget/panel binding are real.
"""
import argparse
import json
from pathlib import Path
import re
import runpy
import subprocess
import tempfile
import time

HERE = Path(__file__).resolve().parent
prepare = runpy.run_path(str(HERE / "panel-benchmark.py"))["prepare"]
resources = runpy.run_path(str(HERE / "service-benchmark.py"))

QML = """
import QtQuick
import Quickshell
import Quickshell.Io
import "plugin" as Calendar
Item {
    id: test
    property var widget
    property double started: 0
    property bool pendingFrame: false
    property int stage: 0
    QtObject {
        id: fixtureService
        property var accounts: [{id: "0", label: "Personal"}]
        property var items: []
        property bool connected: true
        property string status: "ready"
        property string selectedDate: new Date().toLocaleDateString(Qt.locale("en_US"), "yyyy-MM-dd")
        property string accountFilter: "all"
        function selectDate(value) { selectedDate = value }
        function loadRange(start, end) {}
    }
    QtObject { id: providedShell; function serviceFor(id) { return fixtureService } }
    QtObject {
        id: fixtureBar
        property var shell: providedShell
        property bool vertical: false
        property int barSize: 32
        property color barForeground: "white"
        property color foreground: "white"
        property color urgent: "red"
        property string fontFamily: "monospace"
        property bool foregroundAnimationEnabled: false
        function hideTooltip(item) {}
    }
    Component {
        id: component
        Calendar.BarWidget {
            bar: fixtureBar
            property int frameSerial: 0
            function frameReady() {
                if (test.pendingFrame) {
                    test.pendingFrame = false
                    console.log("FRAME " + JSON.stringify({stage: test.stage, elapsed_ms: Date.now() - test.started}))

                }
            }
        }
    }
    Component.onCompleted: {
        var items = []
        var month = fixtureService.selectedDate.slice(0, 8)
        for (var i = 0; i < COUNT; ++i) {
            var day = month + String(1 + i % 28).padStart(2, "0")
            items.push({id: String(i), accountId: "0", title: "Event " + i,
                start: day + "T10:00:00", end: day + "T11:00:00", color: "#83a598"})
        }
        fixtureService.items = items
        var start = Date.now()
        widget = component.createObject(test)
        if (!widget) { console.error("BENCH_FAILED: widget creation"); Qt.exit(1); return }
        console.log("CREATED " + (Date.now() - start))
        console.log("READY 0")
    }
    IpcHandler {
        target: "widgetBenchmark"
        function verifySettings(): void {
            for (var i = 0; i < test.widget.children.length; ++i) {
                var panel = test.widget.children[i].item
                if (panel && panel.persistSettings) {
                    panel.persistSettings({weekStartDay: 0})
                    test.widget.cycleFormat()
                    if (panel.settings !== test.widget.settings) {
                        console.error("BENCH_FAILED: panel settings binding lost")
                        Qt.exit(1)
                        return
                    }
                    console.log("VERIFIED")
                    return
                }
            }
            console.error("BENCH_FAILED: panel not found")
            Qt.exit(1)
        }
        function advance(): void {
            test.stage++
            test.started = Date.now()
            if (test.stage % 2) {
                test.pendingFrame = true
                test.widget.open()
                test.widget.frameSerial++
            } else {
                test.widget.close()
                console.log("READY " + test.stage)
            }
        }
    }
}
"""


def run(args):
    with tempfile.TemporaryDirectory(prefix="calendar-widget-bench-") as directory:
        base = Path(directory)
        env = prepare(base, args.ref)
        (base / "shell.qml").write_text(QML.replace("COUNT", str(args.items)))
        log = base / "output.log"
        with log.open("w") as handle:
            process = subprocess.Popen(["quickshell", "--no-color", "--path", str(base / "shell.qml")],
                                       env=env, stdout=handle, stderr=subprocess.STDOUT)
            try:
                resources["wait_for"](log, "READY 0", process)
                result = {"revision": args.ref, "items": args.items, "states": {}}
                for stage, name in enumerate(("unopened", "open", "closed_after_use", "reopened")):
                    if stage:
                        subprocess.run(["quickshell", "--path", str(base / "shell.qml"), "ipc", "call",
                                        "widgetBenchmark", "advance"], env=env, check=True,
                                       capture_output=True, timeout=10)
                        resources["wait_for"](log, f'"stage":{stage}' if stage % 2 else f"READY {stage}", process)
                    time.sleep(1)
                    result["states"][name] = resources["measure"]({"shell": process}, args.seconds)
                subprocess.run(["quickshell", "--path", str(base / "shell.qml"), "ipc", "call",
                                "widgetBenchmark", "verifySettings"], env=env, check=True,
                               capture_output=True, timeout=10)
                resources["wait_for"](log, "VERIFIED", process)
                output = log.read_text()
                result["create_ms"] = int(re.search(r"CREATED (\d+)", output)[1])
                result["open_frames"] = [json.loads(line) for line in re.findall(r"FRAME (\{.*\})", output)]
                if re.search(r"Binding loop|ReferenceError|TypeError", output):
                    raise RuntimeError(output)
                return result
            finally:
                process.terminate()
                process.wait(timeout=5)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ref", default="working")
    parser.add_argument("--items", type=int, default=1000)
    parser.add_argument("--seconds", type=float, default=10)
    args = parser.parse_args()
    print(json.dumps(run(args), indent=2))
