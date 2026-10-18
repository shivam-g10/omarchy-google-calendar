// IMPORTS

QtObject {
    id: root
    property var labelLocale: Qt.locale("en_US")
    property string activeFormat: "dddd {ordinal} MMMM 'W'ww HH:mm"
    property date selectedDate: new Date(Date.UTC(2026, 9, 18))
    property date clockDate: new Date(2026, 9, 18, 7, 6)
    property var item: ({type: "event", sourceLabels: ["Work"], calendarLabel: "Work",
        reminders: [0, {minutes: 90}], location: "Office"})
    property int checksum: 0

    // HELPERS

    function measure(name, operation, iterations) {
        var total = 0
        for (var warm = 0; warm < 500; warm++) total += operation().length
        var samples = []
        for (var sample = 0; sample < 11; sample++) {
            var start = Date.now()
            for (var n = 0; n < iterations; n++) total += operation().length
            samples.push((Date.now() - start) * 1000 / iterations)
        }
        checksum += total
        samples.sort(function(a, b) { return a - b })
        console.warn("LOCALE_BENCHMARK " + JSON.stringify({locale: labelLocale.name, path: name,
            iterations: iterations, samples: samples.length, medianMicroseconds: samples[5],
            maxMicroseconds: samples[10]}))
    }

    Component.onCompleted: {
        for (var locale of ["en_US", "en_GB", "es_CL", "fr_FR", "de_DE", "pt_BR", "hi_IN", "zh_CN", "ja_JP"]) {
            labelLocale = Qt.locale(locale)
            measure("translation", function() { return root.tr("Complete Google sign-in for {account} in your browser.", {account: "person@example.test"}) }, 10000)
            measure("panel-date", function() { return root.selectedDateLabel() }, 3000)
            measure("reminders", function() { return root.agendaMeta(item) }, 3000)
            measure("bar-format", function() { return root.formatted(clockDate) }, 3000)
        }
        console.warn("LOCALE_BENCHMARK_DONE " + checksum)
        Qt.quit()
    }
}
