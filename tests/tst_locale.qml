import QtQuick
import QtTest
import "../I18n.js" as I18n
import "../Model.js" as Model

TestCase {
    name: "CalendarLocale"

    function spanish(source, values) {
        return I18n.translate("es_CL", source, values)
    }

    function test_locale_selection() {
        compare(I18n.localeName("", "es_CL"), "es_CL")
        compare(I18n.localeName("en-GB", "es_CL"), "en_GB")
        compare(I18n.localeName("", "C"), "en_US")
        compare(I18n.localeName(null, ""), "en_US")
    }

    function test_catalog_fallback_and_values() {
        compare(spanish("Calendar"), "Calendario")
        compare(I18n.translate("es_MX", "All accounts"), "Todas las cuentas")
        compare(I18n.translate("en_US", "Calendar"), "Calendar")
        compare(I18n.translate("fr_FR", "Calendar"), "Calendar")
        compare(spanish("Unknown server detail"), "Unknown server detail")
        compare(spanish("{count} item", {count: 1}), "1 elemento")
        compare(spanish("{count} items", {count: 2}), "2 elementos")
        compare(spanish("Complete Google sign-in for {account} in your browser.", {account: "person@example.test"}),
                "Completa el inicio de sesión de Google para person@example.test en tu navegador.")
        compare(spanish("Calendar service is unavailable"), "El servicio de calendario no está disponible")
        compare(spanish("Calendar service returned invalid data"), "El servicio de calendario devolvió datos no válidos")
        compare(I18n.translate("en_US", "Calendar service is unavailable"), "Calendar service is unavailable")
    }

    function test_reminders() {
        compare(Model.reminderSummary([0, {minutes: 90}, {minutes: 1501}], spanish),
                "Al comenzar, 1h 30m antes, 1d 1h 1m antes")
        compare(Model.reminderSummary([0, {minutes: 90}, {minutes: 1501}]),
                "At start, 1h 30m before, 1d 1h 1m before")
        compare(Model.reminderSummary([0, 0, "Personal reminder", {label: "Custom label"}], spanish),
                "Al comenzar, Personal reminder, Custom label")
        compare(Model.reminderSummary([-1, {minutes: "invalid"}], spanish), "")
    }

    function test_ordinals() {
        compare(Model.ordinalDay(1, "en_US"), "1st")
        compare(Model.ordinalDay(2, "en-GB"), "2nd")
        compare(Model.ordinalDay(3), "3rd")
        compare(Model.ordinalDay(11, "en_US"), "11th")
        compare(Model.ordinalDay(21, "es_CL"), "21")
        compare(Model.ordinalDay(21, "fr_FR"), "21")
        compare(Model.ordinalDay(21, "C"), "21st")
    }
}
