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
        compare(I18n.translate("it_IT", "Calendar"), "Calendar")
        compare(spanish("Unknown server detail"), "Unknown server detail")
        compare(spanish("{count} item", {count: 1}), "1 elemento")
        compare(spanish("{count} items", {count: 2}), "2 elementos")
        compare(spanish("Complete Google sign-in for {account} in your browser.", {account: "person@example.test"}),
                "Completa el inicio de sesión de Google para person@example.test en tu navegador.")
        compare(spanish("Calendar service is unavailable"), "El servicio de calendario no está disponible")
        compare(spanish("Calendar service returned invalid data"), "El servicio de calendario devolvió datos no válidos")
        compare(I18n.translate("en_US", "Calendar service is unavailable"), "Calendar service is unavailable")
    }

    function test_catalogs_are_complete_and_preserve_placeholders() {
        var keys = Object.keys(I18n.catalogs.es).sort()
        compare(keys.length, 60)
        for (var language of ["es", "fr", "de", "pt", "hi", "zh", "ja"]) {
            var catalog = I18n.catalogs[language]
            compare(JSON.stringify(Object.keys(catalog).sort()), JSON.stringify(keys), language)
            for (var source of keys) {
                verify(typeof catalog[source] === "string" && catalog[source].length > 0, language + ": " + source)
                compare(JSON.stringify((catalog[source].match(/\{[^{}]+\}/g) || []).sort()),
                        JSON.stringify((source.match(/\{[^{}]+\}/g) || []).sort()), language + ": " + source)
            }
        }
    }

    function test_supported_languages_and_fallback() {
        var labels = {en_US: "Calendar", en_GB: "Calendar", es_CL: "Calendario", fr_FR: "Calendrier", de_DE: "Kalender",
            pt_BR: "Calendário", hi_IN: "कैलेंडर", zh_CN: "日历", ja_JP: "カレンダー"}
        for (var locale in labels) compare(I18n.translate(locale, "Calendar"), labels[locale], locale)
        compare(I18n.translate("en-GB", "Calendar"), "Calendar")
        compare(I18n.translate("pt_PT", "Calendar"), "Calendário")
        compare(I18n.translate("zh_Hans_CN", "Calendar"), "日历")
        compare(I18n.translate("zh_SG", "Calendar"), "日历")
        for (var unsupported of ["it_IT", "zh_TW", "zh_HK", "zh_MO", "zh_Hant", "unknown"])
            compare(I18n.translate(unsupported, "Calendar"), "Calendar", unsupported)
        for (var language of Object.keys(I18n.catalogs)) {
            compare(I18n.translate(language, "Unknown detail"), "Unknown detail")
            var message = I18n.translate(language, "Complete Google sign-in for {account} in your browser.", {account: "person@example.test"})
            verify(message.indexOf("person@example.test") >= 0)
            verify(message.indexOf("{account}") < 0)
        }
    }

    function test_british_locale_dates() {
        var british = Qt.locale(I18n.localeName("en-GB", "en_US"))
        compare(british.name, "en_GB")
        compare(british.firstDayOfWeek, 1)
        compare(british.dayName(0, Locale.LongFormat), "Sunday")
        compare(new Date(2026, 9, 18, 12).toLocaleDateString(british, Locale.ShortFormat), "18/10/2026")
        compare(Model.ordinalDay(18, british.name), "18th")
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
