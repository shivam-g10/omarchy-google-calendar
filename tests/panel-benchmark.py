#!/usr/bin/env python3
"""Render the production panel with Omarchy controls and synthetic calendar data.

Only the compositor-facing KeyboardPanel is replaced by an offscreen Qt Window.
No desktop configuration, account data, or calendar daemon is used. Frame times
are action-to-frameSwapped latency with the software renderer, not GPU timings.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
HOST = Path("/usr/share/omarchy/shell")

WINDOW = """
import QtQuick
Window {
    property var anchorItem
    property var owner
    property var bar
    property bool open: false
    property bool centerOnBar
    property var focusTarget
    property int contentWidth: 840
    property int contentHeight: 680
    width: contentWidth
    height: contentHeight
    visible: open
    color: "#101315"
    Rectangle { width: 1; height: 1; color: owner && owner.frameSerial % 2 ? "white" : "black" }
    function fittedContentWidth(value) { return Math.min(value, 840) }
    function fittedContentHeight(value) { return Math.min(value, 900) }
    onFrameSwapped: if (owner && owner.frameReady) owner.frameReady()
}
"""

HARNESS = """
import QtQuick
import Quickshell
import "plugin" as Calendar

Item {
    id: test
    property var panel: null
    property var samples: ({})
    property int phase: 0
    property int iteration: -5
    property double started: 0
    property bool waiting: false
    property var operations: ["items", "selection", "month", "language"]
    property int count: COUNT
    property string screenshot: SCREENSHOT

    QtObject {
        id: fixtureService
        property var accounts: [{id: "0", label: "Personal"}, {id: "1", label: "Work"}]
        property var items: []
        property bool connected: true
        property string status: "ready"
        property string selectedDate: "2026-10-18"
        property string accountFilter: "all"
        function loadRange(start, end) {}
        function selectDate(value) { selectedDate = value }
        function setAccountFilter(value) { accountFilter = value }
    }

    Component {
        id: component
        Calendar.Panel {
            property int frameSerial: 0
            calendarService: fixtureService
            settings: ({uiLanguage: "en_US"})
            function frameReady() { test.frameReady() }
        }
    }

    function fixture(offset) {
        var result = []
        for (var i = 0; i < count; ++i) {
            var day = String(1 + i % 28).padStart(2, "0")
            result.push({id: String(i), accountId: String(i % 2), type: "event",
                title: "Calendar event " + (i + offset), accountLabel: i % 2 ? "Work" : "Personal",
                start: "2026-10-" + day + "T10:00:00", end: "2026-10-" + day + "T11:00:00",
                color: i % 2 ? "#83a598" : "#fabd2f"})
        }
        return result
    }

    function fail(message) { console.error("FAIL " + message); Qt.exit(1) }

    function run() {
        if (phase === operations.length) {
            if (!panel.agendaItems.length) { fail("selected agenda must contain fixture events: " + panel.selectedDateKey + ", " + fixtureService.items.length); return }
            console.log("BENCHMARK " + JSON.stringify(samples))
            if (screenshot) {
                panel.settings = {uiLanguage: screenshot.split("/").pop().split(".")[0]}
                capture.restart()
            } else Qt.quit()
            return
        }
        var operation = operations[phase]
        started = Date.now()
        waiting = true
        if (operation === "items") fixtureService.items = fixture(iteration)
        else if (operation === "selection") panel.selectDateKey(iteration % 2 ? "2026-10-18" : "2026-10-19")
        else if (operation === "month") panel.moveMonth(iteration % 2 ? -1 : 1)
        else panel.settings = {uiLanguage: iteration % 2 ? "en_US" : "es_CL"}
        panel.frameSerial++
    }

    function frameReady() {
        if (!waiting) return
        waiting = false
        var elapsed = Date.now() - started
        var operation = operations[phase]
        if (iteration >= 0) {
            if (!samples[operation]) samples[operation] = []
            samples[operation].push(elapsed)
        }
        if (++iteration >= SAMPLES) {
            phase++
            iteration = -5
            panel.viewYear = 2026
            panel.viewMonth = 9
            panel.selectDateKey("2026-10-18")
        }
        next.restart()
    }

    Component.onCompleted: {
        fixtureService.items = fixture(0)
        var start = Date.now()
        panel = component.createObject(test)
        if (!panel) { fail("panel failed to instantiate"); return }
        samples.create_ms = Date.now() - start
        panel.open()
        panel.today = new Date(2026, 9, 18, 12)
        panel.viewYear = 2026
        panel.viewMonth = 9
        panel.selectDateKey("2026-10-18")
        next.start()
    }
    Timer { id: next; interval: 30; onTriggered: test.run() }
    Timer {
        id: capture; interval: 150
        onTriggered: {
            // The platform wrapper is the only Window in this isolated engine.
            for (var i = 0; i < panel.data.length; ++i) {
                var candidate = panel.data[i]
                if (candidate.contentItem) {
                    candidate.contentItem.grabToImage(function(result) {
                        if (!result.saveToFile(screenshot)) fail("screenshot save failed")
                        else Qt.quit()
                    })
                    return
                }
            }
            fail("render window not found")
        }
    }
    Timer { interval: 60000; running: true; onTriggered: test.fail("frame timeout: " + phase + "/" + iteration) }
}
"""


def prepare(base, ref):
    for name in ("Ui", "Commons"):
        shutil.copytree(HOST / name, base / name)
    (base / "Ui/KeyboardPanel.qml").write_text(WINDOW)
    plugin = base / "plugin"
    plugin.mkdir()
    for name in ("BarWidget.qml", "Panel.qml", "Model.js", "I18n.js", "CalendarService.qml",
                 "services/qmldir", "services/FallbackCalendarService.qml"):
        target = plugin / name
        target.parent.mkdir(parents=True, exist_ok=True)
        if ref == "working":
            shutil.copyfile(ROOT / name, target)
        else:
            result = subprocess.run(["git", "show", f"{ref}:{name}"], cwd=ROOT,
                                    capture_output=True, text=True)
            if result.returncode == 0:
                target.write_text(result.stdout)
    runtime = base / "runtime"
    runtime.mkdir(mode=0o700)
    env = os.environ.copy()
    env.update(HOME=str(base / "home"), XDG_RUNTIME_DIR=str(runtime),
               XDG_CONFIG_HOME=str(base / "config"), XDG_CACHE_HOME=str(base / "cache"),
               QT_QPA_PLATFORM="offscreen", QT_QUICK_BACKEND="software",
               QT_QPA_PLATFORMTHEME="", QT_STYLE_OVERRIDE="Fusion",
               QT_FORCE_STDERR_LOGGING="1", TZ="UTC")
    for key in ("WAYLAND_DISPLAY", "DISPLAY", "HYPRLAND_INSTANCE_SIGNATURE"):
        env.pop(key, None)
    return env


def run(ref, count, samples, screenshot):
    with tempfile.TemporaryDirectory(prefix="calendar-panel-bench-") as directory:
        base = Path(directory)
        env = prepare(base, ref)
        source = HARNESS.replace("COUNT", str(count)).replace("SAMPLES", str(samples))
        source = source.replace("SCREENSHOT", json.dumps(str(screenshot) if screenshot else ""))
        (base / "shell.qml").write_text(source)
        result = subprocess.run(["quickshell", "--no-color", "--path", str(base / "shell.qml")],
                                env=env, capture_output=True, text=True, timeout=70)
        output = result.stdout + result.stderr
        match = re.search(r"BENCHMARK (\{.*\})", output)
        if result.returncode or not match or re.search(r"FAIL |Binding loop|ReferenceError|TypeError", output):
            raise RuntimeError(output)
        data = json.loads(match[1])
        return {"revision": ref, "items": count, "samples": samples,
                "create_ms": data.pop("create_ms"),
                "frames": {key: {"median_ms": sorted(values)[len(values) // 2],
                                 "p95_ms": sorted(values)[min(len(values)-1, int(len(values)*.95))],
                                 "raw_ms": values} for key, values in data.items()}}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ref", default="working")
    parser.add_argument("--items", type=int, default=75)
    parser.add_argument("--samples", type=int, default=30)
    parser.add_argument("--screenshot", type=Path)
    args = parser.parse_args()
    print(json.dumps(run(args.ref, args.items, args.samples, args.screenshot), indent=2))
