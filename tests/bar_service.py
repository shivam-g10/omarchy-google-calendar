"""Exercise actual bar service selection in an isolated, headless QML engine.

Only presentation types and the calendar socket client are replaced with mocks.
The test never loads desktop configuration or connects to the calendar daemon.
"""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]

MOCKS = {
    "Ui/BarWidget.qml": """
import QtQuick
Item {
    property QtObject bar: null
    property string moduleName: ""
    property var settings: ({})
    readonly property bool vertical: false
    function setting(name, fallback) {
        return settings[name] === undefined ? fallback : settings[name]
    }
    function broadcast(method) { this[method]() }
}
""",
    "Ui/WidgetButton.qml": """
import QtQuick
Item {
    property var bar
    property string text
    property bool labelVisible
    property bool hasVisualContent
    property real fixedHeight
    property real horizontalMargin
    property real verticalPadding
    property real labelWidth: 10
    property string fontFamily: "sans"
    property real fontSize: 12
    property color foreground: "white"
    signal pressed(int button)
}
""",
    "Ui/OpticalGlyph.qml": """
import QtQuick
Item {
    property string text
    property string fontFamily
    property real fontSize
    property color color
}
""",
    "Commons/qmldir": "singleton Style 1.0 Style.qml\n",
    "Commons/Style.qml": """
pragma Singleton
import QtQuick
QtObject {
    property var bar: ({iconSlot: 16})
    function space(value) { return value }
}
""",
    "plugin/Panel.qml": """
import QtQuick
Item {
    objectName: "calendar-panel"
    property string moduleName
    property string ipcTarget
    property var bar
    property var settings
    property var anchorItem
    property var hostWidget
    property var calendarService
    property var service
    property bool opened: false
    function open() { opened = true }
    function close() { opened = false }
    function toggle() { opened = !opened }
}
""",
    "plugin/CalendarService.qml": """
import QtQuick
Item {
    property string marker: "fallback"
    property string accountFilter: "all"
    Component.onCompleted: console.log("FALLBACK_CREATED")
    Component.onDestruction: console.log("FALLBACK_DESTROYED")
}
""",
}

HARNESS = """
import QtQuick
import Quickshell
import "plugin" as Calendar

Item {
    id: test
    property int stage: 0
    property var widgets: []
    property var savedFallback: null

    QtObject { id: shared; property string marker: "injected" }
    QtObject {
        id: providedShell
        property var currentService: shared
        function serviceFor(id) {
            check(id === "shivam.clock", "service lookup must keep plugin scope")
            return currentService
        }
    }
    QtObject { id: missingLookupShell }
    QtObject { id: nullLookupShell; function serviceFor(id) { return null } }
    QtObject { id: providedBar; property var shell: providedShell }
    QtObject { id: missingLookupBar; property var shell: missingLookupShell }
    QtObject { id: nullLookupBar; property var shell: nullLookupShell }
    QtObject { id: missingShellBar; property var shell: null }

    Component { id: widget; Calendar.BarWidget {} }

    function check(value, message) {
        if (!value) {
            console.error("TEST_FAILED: " + message)
            Qt.exit(1)
            throw new Error(message)
        }
    }

    function create(bar) {
        var item = widget.createObject(test, {bar: bar})
        check(item !== null, "widget must load")
        return item
    }

    function panel(item) {
        for (var i = 0; i < item.children.length; ++i) {
            var child = item.children[i]
            if (child.item && child.item.objectName === "calendar-panel") return child.item
        }
        check(false, "panel must load")
    }

    function tick() {
        SCENARIO
    }

    Timer { interval: 20; running: true; repeat: true; onTriggered: test.tick() }
    Timer {
        interval: 3000; running: true
        onTriggered: { console.error("TEST_FAILED: timeout"); Qt.exit(1) }
    }
}
"""


@unittest.skipUnless(shutil.which("quickshell"), "quickshell is required")
class BarServiceTests(unittest.TestCase):
    def run_qml(self, scenario, expected_fallbacks):
        with tempfile.TemporaryDirectory(prefix="calendar-bar-test-") as directory:
            base = Path(directory)
            for relative, content in MOCKS.items():
                path = base / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content)
            for relative in ("BarWidget.qml", "Model.js", "services/qmldir",
                             "services/FallbackCalendarService.qml"):
                path = base / "plugin" / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / relative, path)
            # Copy helper imports added by other bar features without replacing mocks.
            for source in ROOT.glob("*.js"):
                shutil.copyfile(source, base / "plugin" / source.name)
            (base / "shell.qml").write_text(HARNESS.replace("SCENARIO", scenario))
            runtime = base / "runtime"
            runtime.mkdir(mode=0o700)
            env = os.environ.copy()
            env.update(QT_QPA_PLATFORM="offscreen", QT_QPA_PLATFORMTHEME="",
                       QT_QUICK_BACKEND="software", QT_STYLE_OVERRIDE="Fusion",
                       XDG_RUNTIME_DIR=str(runtime), XDG_CONFIG_HOME=str(base / "config"),
                       XDG_CACHE_HOME=str(base / "cache"), HOME=str(base / "home"))
            env.pop("WAYLAND_DISPLAY", None)
            env.pop("DISPLAY", None)
            result = subprocess.run(
                ["quickshell", "--no-color", "--path", str(base / "shell.qml")],
                env=env, capture_output=True, text=True, timeout=10,
            )
            output = result.stdout + result.stderr
            self.assertEqual(result.returncode, 0, output)
            self.assertIn("TEST_PASSED", output)
            self.assertNotIn("TEST_FAILED", output)
            self.assertNotIn("Binding loop", output)
            self.assertEqual(output.count("FALLBACK_CREATED"), expected_fallbacks, output)
            self.assertNotIn("FALLBACK_DESTROYED", output)

    def test_injected_service_and_unattached_widgets_do_not_create_fallback(self):
        self.run_qml("""
        if (stage++ === 0) {
            widgets = [create(providedBar), create(missingShellBar), create(null)]
            return
        }
        check(widgets[0].calendarService === shared, "injected service must win")
        check(panel(widgets[0]).calendarService === shared, "panel must receive service")
        check(widgets[1].calendarService === null, "wait for shell injection")
        check(widgets[2].calendarService === null, "wait for bar injection")
        console.log("TEST_PASSED")
        Qt.quit()
        """, 0)

    def test_fallback_is_shared_and_survives_widget_lifecycle(self):
        self.run_qml("""
        if (stage === 0) {
            widgets = [create(missingLookupBar), create(nullLookupBar)]
        } else if (stage === 1) {
            savedFallback = widgets[0].calendarService
            check(savedFallback && savedFallback.marker === "fallback", "missing lookup fallback")
            check(widgets[1].calendarService === savedFallback, "null lookup shares one client")
            check(panel(widgets[0]).calendarService === savedFallback, "panel receives fallback")
            savedFallback.accountFilter = "retained"
            widgets[0].open()
            widgets[0].close()
            widgets[0].destroy()
            widgets = [widgets[1]]
        } else if (stage === 2) {
            check(widgets[0].calendarService.accountFilter === "retained", "other monitor remains usable")
            widgets.push(create(nullLookupBar))
        } else if (stage === 3) {
            check(widgets[1].calendarService === savedFallback, "recreated widget reuses client")
            widgets[0].bar = providedBar
        } else if (stage === 4) {
            check(widgets[0].calendarService === shared, "late injected service must win")
            check(panel(widgets[0]).calendarService === shared, "panel follows service replacement")
            providedShell.currentService = null
        } else if (stage === 5) {
            check(widgets[0].calendarService === savedFallback, "lost service uses same fallback")
            console.log("TEST_PASSED")
            Qt.quit()
        }
        stage++
        """, 1)


if __name__ == "__main__":
    unittest.main()
