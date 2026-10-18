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


    def test_all_supported_clock_locales(self):
        self.run_qml("""
        if (stage++ === 0) {
            var languages = ["en_US", "en-GB", "es_CL", "fr_FR", "de_DE", "pt_BR", "hi_IN", "zh_CN", "ja_JP"]
            for (var language of languages) {
                var item = create(providedBar)
                item.settings = {uiLanguage: language, format: "dddd {ordinal} MMMM 'W'ww HH:mm"}
                widgets.push(item)
            }
            return
        }
        var date = new Date(2026, 9, 18, 7, 6)
        for (var widget of widgets) {
            var locale = widget.labelLocale
            var ordinal = locale.name.indexOf("en_") === 0 ? "18th" : "18"
            var expected = locale.dayName(0, Locale.LongFormat) + " " + ordinal + " "
                + locale.monthName(9, Locale.LongFormat) + " W42 07:06"
            check(widget.formatted(date) === expected, locale.name + " named custom pattern: " + widget.formatted(date))
            widget.settings = {uiLanguage: locale.name, format: "yyyy-MM-dd 'literal' HH:mm"}
            check(widget.formatted(date) === "2026-10-18 literal 07:06", locale.name + " numeric/literal pattern")
        }
        console.log("TEST_PASSED")
        Qt.quit()
        """, 0)


if __name__ == "__main__":
    unittest.main()
