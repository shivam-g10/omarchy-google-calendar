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
    # The real UI module also exports Panel. Keep this collision in the fixture
    # so an unqualified Panel cannot silently resolve to the calendar component.
    "Ui/Panel.qml": """
import QtQuick
Item {
    property string moduleName
    property string ipcTarget
    property var bar
    property var settings
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
    property int refreshCalls: 0
    property int weekStartChanges: 0
    function open() { opened = true }
    function close() { opened = false }
    function toggle() { opened = !opened }
    function refreshFromGoogle() { refreshCalls++; calendarService.refresh() }
    function toggleWeekStart() { weekStartChanges++ }
}
""",
    "plugin/CalendarService.qml": """
import QtQuick
Item {
    property string marker: "fallback"
    property string accountFilter: "all"
    property int refreshCalls: 0
    function refresh() { refreshCalls++; return true }
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
    property var savedPanel: null

    QtObject {
        id: shared
        property string marker: "injected"
        property int refreshCalls: 0
        function refresh() { refreshCalls++; return true }
    }
    QtObject {
        id: replacementShared
        property int refreshCalls: 0
        function refresh() { refreshCalls++; return true }
    }
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
    QtObject { id: providedBarCopy; property var shell: providedShell }
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
        return null
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
        if (stage === 0) {
            widgets = [create(providedBar), create(missingShellBar), create(null)]
        } else if (stage === 1) {
            check(widgets[0].calendarService === shared, "injected service must win")
            check(panel(widgets[0]) === null, "unopened widget must not load panel")
            check(widgets[1].calendarService === null, "wait for shell injection")
            check(widgets[2].calendarService === null, "wait for bar injection")
            widgets[0].syncCalendars()
            check(shared.refreshCalls === 1, "refresh before first open reaches service")
            check(panel(widgets[0]) === null, "refresh must not create panel")
            widgets[0].open()
        } else if (stage === 2) {
            savedPanel = panel(widgets[0])
            check(savedPanel !== null && savedPanel.opened, "first open loads and opens panel")
            check(savedPanel.calendarService === shared, "panel receives service")
            check(savedPanel.hostWidget === widgets[0] && savedPanel.anchorItem !== null, "panel host and anchor bound")
            widgets[0].settings = {format: "HH:mm", uiLanguage: "es_CL"}
            widgets[0].bar = providedBarCopy
            providedShell.currentService = replacementShared
        } else if (stage === 3) {
            check(savedPanel.settings === widgets[0].settings, "settings binding follows replacement")
            check(savedPanel.bar === providedBarCopy, "bar binding follows replacement")
            check(savedPanel.calendarService === replacementShared, "service binding follows replacement")
            widgets[0].close()
            check(!widgets[0].opened && panel(widgets[0]) === savedPanel, "close retains loaded panel")
            widgets[0].syncCalendars()
            check(savedPanel.refreshCalls === 1 && replacementShared.refreshCalls === 1, "loaded panel refresh path")
            widgets[0].togglePanel()
            check(widgets[0].opened && panel(widgets[0]) === savedPanel, "toggle reuses loaded panel")
            console.log("TEST_PASSED")
            Qt.quit()
        }
        stage++
        """, 0)

    def test_fallback_is_shared_and_survives_widget_lifecycle(self):
        self.run_qml("""
        if (stage === 0) {
            widgets = [create(missingLookupBar), create(nullLookupBar)]
        } else if (stage === 1) {
            check(panel(widgets[0]) === null && panel(widgets[1]) === null, "replacement widgets start without panels")
            widgets[0].syncCalendars()
            savedFallback = widgets[0].calendarService
            check(savedFallback && savedFallback.marker === "fallback", "missing lookup fallback")
            check(widgets[1].calendarService === savedFallback, "null lookup shares one client")
            check(savedFallback.refreshCalls === 1, "fallback refresh before first open")
            check(panel(widgets[0]) === null, "fallback refresh does not create panel")
            savedFallback.accountFilter = "retained"
            widgets[0].open()
            widgets[1].toggleWeekStart()
        } else if (stage === 2) {
            savedPanel = panel(widgets[0])
            check(savedPanel.calendarService === savedFallback, "panel receives fallback")
            check(panel(widgets[1]).weekStartChanges === 1 && !widgets[1].opened, "week-start action loads without opening")
            widgets[0].close()
            check(panel(widgets[0]) === savedPanel, "closed panel retained")
            widgets[0].destroy()
            widgets = [widgets[1]]
        } else if (stage === 3) {
            check(widgets[0].calendarService.accountFilter === "retained", "other monitor remains usable")
            widgets.push(create(nullLookupBar))
            widgets[1].togglePanel()
        } else if (stage === 4) {
            check(widgets[1].calendarService === savedFallback, "recreated widget reuses client")
            check(widgets[1].opened && panel(widgets[1]).calendarService === savedFallback, "first toggle loads panel")
            widgets[0].bar = providedBar
        } else if (stage === 5) {
            check(widgets[0].calendarService === shared, "late injected service must win")
            check(panel(widgets[0]).calendarService === shared, "panel follows service replacement")
            providedShell.currentService = null
        } else if (stage === 6) {
            check(widgets[0].calendarService === savedFallback, "lost service uses same fallback")
            console.log("TEST_PASSED")
            Qt.quit()
        }
        stage++
        """, 1)


if __name__ == "__main__":
    unittest.main()
