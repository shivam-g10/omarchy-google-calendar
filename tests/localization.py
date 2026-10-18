"""Check locale changes against the actual bar in the isolated QML harness."""
import shutil
import unittest

import bar_service


@unittest.skipUnless(shutil.which("quickshell"), "quickshell is required")
class LocaleTests(unittest.TestCase):
    run_qml = bar_service.BarServiceTests.run_qml

    def test_custom_clock_patterns_follow_each_widget_locale(self):
        self.run_qml("""
        if (stage++ === 0) {
            widgets = [create(providedBar), create(providedBar)]
            widgets[0].settings = {uiLanguage: "es-CL", format: "dddd {ordinal} MMMM 'W'ww HH:mm"}
            widgets[1].settings = {uiLanguage: "en_US", format: "dddd {ordinal} MMMM 'W'ww HH:mm"}
            return
        }
        var date = new Date(2026, 9, 18, 7, 6)
        check(widgets[0].labelLocale.name === "es_CL", "Spanish Chile override")
        check(widgets[0].formatted(date) === "domingo 18 octubre W42 07:06", "Spanish names and plain ordinal preserve custom pattern")
        check(widgets[1].formatted(date) === "Sunday 18th October W42 07:06", "English widget retains ordinal independently")
        widgets[0].settings = {uiLanguage: "es_CL", format: "yyyy-MM-dd 'literal' HH:mm"}
        check(widgets[0].formatted(date) === "2026-10-18 literal 07:06", "literal and numeric custom format preserved")
        widgets[0].settings = {format: "yyyy-MM-dd HH:mm"}
        var systemName = Qt.locale().name
        check(widgets[0].labelLocale.name === (systemName === "C" ? "en_US" : systemName), "missing language follows system")
        check(widgets[0].formatted(date) === "2026-10-18 07:06", "removing override preserves format")
        console.log("TEST_PASSED")
        Qt.quit()
        """, 0)


if __name__ == "__main__":
    unittest.main()
