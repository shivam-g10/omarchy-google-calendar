import QtQml
import "../Model.js" as Model

QtObject {
  property var options: ({})
  property var keys: []
  property var items: []
  property real checksum: 0

  function operation(name) {
    if (name === "monthGrid")
      return Model.monthGrid(options.year, options.month - 1, 1, keys[15])[5].days[6].day
    if (name === "localDayBounds42") {
      var total = 0
      for (var key of keys) {
        var bounds = Model.localDayBounds(key)
        total += bounds.end - bounds.start
      }
      return total
    }
    var index = Model.agendaIndex(items, keys[0], keys[41], "all")
    return index[keys[0]].count
  }

  function batch(name, count) {
    var sum = 0
    var start = Date.now()
    for (var i = 0; i < count; i++) sum += operation(name)
    var elapsed = Date.now() - start
    checksum += sum
    return elapsed
  }

  function measure(name) {
    var count = 1
    var calibration = batch(name, count)
    while (calibration < options.targetMs && count < 8192) {
      count *= 2
      calibration = batch(name, count)
    }
    for (var warmup = 0; warmup < options.warmups; warmup++) batch(name, count)
    var values = []
    for (var sample = 0; sample < options.samples; sample++) values.push(batch(name, count) / count)
    values.sort(function(a, b) { return a - b })
    console.warn("MODEL_BENCH " + JSON.stringify({
      kind: "result", operation: name, timezone: options.timezone,
      year: options.year, month: options.month, weekStart: 1,
      firstKey: keys[0], lastKey: keys[41], days: keys.length,
      items: name === "agendaIndex1000" ? items.length : 0,
      fixture: "1000 one-hour timed events across 42 dates; every tenth St Johns event uses the resumed hour",
      samples: options.samples, iterationsPerBatch: count, calibrationMs: calibration,
      p50Ms: values[Math.ceil(values.length * 0.5) - 1],
      p95Ms: values[Math.ceil(values.length * 0.95) - 1], checksum: checksum
    }))
  }

  Component.onCompleted: {
    try {
      var weeks = Model.monthGrid(options.year, options.month - 1, 1, "")
      var dates = []
      for (var week of weeks) for (var day of week.days) dates.push(day.key)
      keys = dates
      var events = []
      for (var i = 0; i < 1000; i++) {
        var key = keys[i % keys.length]
        var event = { id: String(i), type: "event", accountId: String(i % 3), title: "Event " + i,
          start: key + "T10:00:00", end: key + "T11:00:00", color: ["red", "blue", "green"][i % 3] }
        if (options.timezone === "America/St_Johns" && i % 10 === 0) {
          event.start = "2009-10-31T23:10:00-03:30"
          event.end = "2009-10-31T23:20:00-03:30"
        }
        events.push(event)
      }
      items = events
      for (var name of ["monthGrid", "localDayBounds42", "agendaIndex1000"]) measure(name)
      Qt.quit()
    } catch (error) {
      console.error("MODEL_BENCH_ERROR " + error)
      Qt.exit(1)
    }
  }
}
