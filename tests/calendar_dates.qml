import QtQml
import "../Model.js" as Model

QtObject {
  id: root
  property string zone: "UTC"
  property QtObject service: ServiceProbe {}
  property QtObject panel: PanelProbe { actualService: root.service }

  function check(condition, message) {
    if (!condition) throw new Error(message)
  }

  function checkBounds(key, start, end) {
    var bounds = Model.localDayBounds(key)
    check(bounds.start === Date.parse(start), key + " start: " + new Date(bounds.start).toISOString())
    check(bounds.end === Date.parse(end), key + " end: " + new Date(bounds.end).toISOString())
    return bounds
  }

  function run() {
    // Include the first-of-month Havana gap, leap years, and year rollovers.
    for (var year of [2000, 2001, 2026, 2027]) {
      for (var month = 0; month < 12; month++) {
        for (var start of [0, 1]) {
          var today = Model.dateKey(year, month, 15)
          var weeks = Model.monthGrid(year, month, start, today)
          check(weeks.length === 6, "six weeks")
          var seen = {}, previous = null, markedToday = 0, inMonth = 0
          for (var week of weeks) {
            check(week.days.length === 7, "seven weekdays")
            for (var column = 0; column < 7; column++) {
              var cell = week.days[column]
              check(!seen[cell.key], "duplicate " + cell.key)
              seen[cell.key] = true
              check(cell.weekday === (start + column) % 7, "weekday column " + cell.key)
              check(cell.key === Model.dateKey(cell.year, cell.month, cell.day), "display/key disagreement")
              var ordinal = Date.UTC(cell.year, cell.month, cell.day) / 86400000
              check(previous === null || ordinal === previous + 1, "noncontiguous " + cell.key)
              previous = ordinal
              check(cell.weekend === (cell.weekday === 0 || cell.weekday === 6), "weekend")
              check(cell.inMonth === (cell.year === year && cell.month === month), "month membership")
              if (cell.inMonth) inMonth++
              if (cell.today) markedToday++
              if (cell.weekday === 4) check(week.week === Model.isoWeek(cell.year, cell.month, cell.day), "ISO week")
            }
          }
          var leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0)
          check(inMonth === [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][month], "month length")
          check(markedToday === 1, "today marker")
        }
      }
    }
    var september = Model.monthGrid(2026, 8, 1, "2026-09-28")
    check(JSON.stringify(september[0].days.map(function(cell) { return cell.day })) === "[31,1,2,3,4,5,6]", "September row")
    check(september[4].days[0].key === "2026-09-28", "September 28 is Monday")
    check(september[0].week === 36 && september[4].week === 40, "September ISO weeks")

    check(service.selectDate("2026-09-06"), "service accepts missing-midnight date")
    check(service.selectedDate === "2026-09-06", "service preserves date identity")
    check(service.agendaStart === "2026-08-25" && service.agendaEnd === "2026-10-08", "service month range")
    check(!service.selectDate("2026-02-30"), "invalid date rejected")
    check(panel.selectedDateLabel() === "Sunday, 6th September", "selected date label")
    panel.viewMonth = 7
    panel.weekStart = 0
    panel.requestRange()
    check(service.agendaStart === "2026-07-26" && service.agendaEnd === "2026-09-06", "exclusive grid end in midnight gap")
    panel.selectedDateKey = "2026-08-31"
    panel.moveMonth(1)
    check(panel.selectedDateKey === "2026-09-30", "month navigation clamps day")
    check(JSON.stringify(Model.stepMonth(2001, 2, 1)) === '{"year":2001,"month":3}', "month navigation across first-day gap")

    if (zone === "America/Santiago") {
      var bounds = checkBounds("2026-09-06", "2026-09-06T04:00:00Z", "2026-09-07T03:00:00Z")
      var timed = { type: "event", start: "2026-09-06T12:00:00-03:00", end: "2026-09-06T13:00:00-03:00" }
      check(Model.itemOccursOnDate(timed, "2026-09-06"), "midday event remains visible")
      check(!Model.itemOccursOnDate(timed, "2026-09-05"), "event does not leak to prior day")
      check(!Model.itemOccursOnDate({ start: "2026-09-05T23:00:00-04:00", end: "2026-09-06T04:00:00Z" }, "2026-09-06"), "end is exclusive")
      check(Model.itemOccursOnDate({ start: "2026-09-06T03:30:00Z", end: "2026-09-06T04:30:00Z" }, "2026-09-06"), "crossing event overlaps gap day")
      check(!Model.itemOccursOnDate({ start: "2026-09-07T03:00:00Z", end: "2026-09-07T04:00:00Z" }, "2026-09-06"), "next-day start excluded")
    } else if (zone === "America/Havana") {
      checkBounds("2001-04-01", "2001-04-01T05:00:00Z", "2001-04-02T04:00:00Z")
      checkBounds("2026-11-01", "2026-11-01T04:00:00Z", "2026-11-02T05:00:00Z")
      check(Model.itemOccursOnDate({ start: "2026-11-01T04:15:00Z", end: "2026-11-01T04:45:00Z" }, "2026-11-01"), "first repeated-midnight hour included")
      check(Model.itemOccursOnDate({ start: "2026-11-01T05:15:00Z", end: "2026-11-01T05:45:00Z" }, "2026-11-01"), "second repeated-midnight hour included")
    } else if (zone === "Asia/Kathmandu") {
      checkBounds("1986-01-01", "1985-12-31T18:30:00Z", "1986-01-01T18:15:00Z")
    } else if (zone === "Pacific/Apia") {
      checkBounds("2011-12-30", "2011-12-30T10:00:00Z", "2011-12-30T10:00:00Z")
      check(!Model.itemOccursOnDate({ start: "2011-12-29T00:00:00Z", end: "2012-01-01T00:00:00Z" }, "2011-12-30"), "skipped civil day has no timed events")
    }
    check(Model.itemOccursOnDate({ type: "task", dueDate: "2026-09-06" }, "2026-09-06"), "task date remains civil")
    check(Model.itemOccursOnDate({ allDay: true, start: "2026-09-06", end: "2026-09-07" }, "2026-09-06"), "all-day date remains civil")
    check(!Model.itemOccursOnDate({ allDay: true, start: "2026-09-06", end: "2026-09-07" }, "2026-09-07"), "all-day end remains exclusive")
    console.warn("PASS calendar dates: " + zone)
  }

  Component.onCompleted: {
    try {
      run()
      Qt.quit()
    } catch (error) {
      console.error("FAIL " + zone + ": " + error)
      Qt.exit(1)
    }
  }
}
