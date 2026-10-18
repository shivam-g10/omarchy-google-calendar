"""Run date regressions in Qt's JS engine without starting the shell or backend."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
QML = shutil.which("qml6") or shutil.which("qml")


def function(source, name):
    match = re.search(rf"^  function {name}\(.*?^  }}", source, re.M | re.S)
    if not match:
        raise AssertionError(f"Missing production function: {name}")
    return match.group(0)


def property_line(source, name):
    match = re.search(rf"^  (?:readonly )?property \w+ {name}:.*(?:\n {{4}}.*)*", source, re.M)
    if not match:
        raise AssertionError(f"Missing production property: {name}")
    return match.group(0)


class CalendarDateTests(unittest.TestCase):
    def test_qml_dates(self):
        self.assertIsNotNone(QML, "Qt QML runner required (qml6 or qml)")
        model_import = f'import {json.dumps((ROOT / "Model.js").as_uri())} as Model\n'
        locale_import = f'import {json.dumps((ROOT / "I18n.js").as_uri())} as I18n\n'
        service = (ROOT / "CalendarService.qml").read_text()
        panel = (ROOT / "Panel.qml").read_text()
        # Isolate the real date methods from shell imports and socket startup.
        # The only stubs are the backend request and unrelated panel state.
        service_probe = "import QtQml\n" + model_import + "QtObject {\n" + "\n".join([
            property_line(service, "selectedDate"),
            'property string agendaStart: ""',
            'property string agendaEnd: ""',
            'function requestAgenda() { return true }',
            'function requestFailed(method, code, message) { throw new Error(message) }',
            *[function(service, name) for name in (
                "dateKey", "monthRange", "selectedRangeLoaded", "selectDate", "loadRange")],
        ]) + "\n}\n"
        panel_probe = "import QtQml\n" + model_import + locale_import + "QtObject {\nid: root\n" + "\n".join([
            'property var actualService: null',
            'property string todayKey: "2026-09-01"',
            'property string selectedDateKey: "2026-09-06"',
            'property string requestedRange: ""',
            'property int viewYear: 2026',
            'property int viewMonth: 8',
            'property int weekStart: 1',
            'property var labelLocale: Qt.locale("en_US")',
            'property var serviceItems: []',
            'property string accountFilter: "all"',
            property_line(panel, "selectedDate"),
            property_line(panel, "weeks"),
            *[property_line(panel, name) for name in (
                "firstVisibleDate", "lastVisibleDate", "agendaIndex", "agendaItems")],
            *[function(panel, name) for name in (
                "tr", "dateFromKey", "selectedDateLabel", "selectDateKey", "requestRange", "moveMonth")],
        ]) + "\n}\n"
        with tempfile.TemporaryDirectory(prefix="calendar-qml-dates-") as directory:
            path = Path(directory)
            (path / "ServiceProbe.qml").write_text(service_probe)
            (path / "PanelProbe.qml").write_text(panel_probe)
            template = (ROOT / "tests/calendar_dates.qml").read_text()
            for zone in ("America/Santiago", "America/Havana", "Asia/Kolkata",
                         "Asia/Kathmandu", "Pacific/Apia", "UTC"):
                with self.subTest(timezone=zone):
                    test = template.replace('import "../Model.js" as Model', model_import.strip())
                    test = test.replace('property string zone: "UTC"', f'property string zone: {json.dumps(zone)}')
                    (path / "test.qml").write_text(test)
                    env = os.environ.copy()
                    env.update(TZ=zone, QT_QPA_PLATFORM="offscreen", QT_FORCE_STDERR_LOGGING="1")
                    result = subprocess.run([QML, "-a", "core", str(path / "test.qml")],
                                            env=env, capture_output=True, text=True, timeout=15)
                    output = result.stdout + result.stderr
                    self.assertEqual(result.returncode, 0, output)
                    self.assertIn("PASS calendar dates", output, output)
                    self.assertNotIn("FAIL", output, output)


if __name__ == "__main__":
    unittest.main()
