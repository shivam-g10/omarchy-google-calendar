// Pure date and format math for the clock widget and its calendar panel.
// Everything here is locale- and Qt-free so it can be unit tested under Node;
// the QML owns month/weekday naming through Qt.locale().

var MS_PER_DAY = 86400000

// Weekday indices match both JS Date.getDay() and QML's Locale.Sunday…
// Locale.Saturday, so a locale's firstDayOfWeek can be passed straight in.
var WEEKDAY_NAMES = ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"]

// ---- Bar label formats. Right-clicking the clock walks these in order and
//      writes the result back to shell.json, so the label the bar shows and
//      the format the config stores are always the same thing.
//
// The locale-shaped time presets are each followed by their 12-hour twin, so
// the walk from a 24-hour label to the same label in AM/PM is a single right
// click rather than a lap of the ring. The ISO preset is deliberately left
// without one: ISO 8601 writes time on a 24-hour clock, so an AM/PM variant
// would contradict the only thing that format is for.
var CLOCK_FORMATS = [
  "dddd, {ordinal} MMM, HH:mm",
  "dddd h:mm AP",
  "HH:mm",
  "h:mm AP",
  "ddd d MMM HH:mm",
  "ddd d MMM h:mm AP",
  "d MMMM 'W'ww yyyy",
  "yyyy-MM-dd HH:mm"
]

// Vertical bars have room for a few stacked lines and nothing else, so the
// ring stays short. AM/PM costs a fourth line, which is why only the plain
// time carries it here.
var VERTICAL_CLOCK_FORMATS = [
  "HH\n—\nmm",
  "h\n—\nmm\nAP",
  "dd\nMMM\n'W'ww\n''yy",
  "HH\nmm"
]

function clockFormats(vertical) {
  return vertical ? VERTICAL_CLOCK_FORMATS.slice() : CLOCK_FORMATS.slice()
}

// The presets in a fixed order, plus the configured alternate and current
// format when they are something else. The order must not depend on which
// entry is current: cycling writes the result back to shell.json, and a ring
// that reshuffled itself around the current value would bounce between two
// entries instead of walking.
function clockFormatRing(configured, configuredAlt, presets) {
  var ring = []
  var candidates = (presets || []).concat([configuredAlt, configured])
  for (var i = 0; i < candidates.length; i++) {
    var format = String(candidates[i] === undefined || candidates[i] === null ? "" : candidates[i])
    if (format === "" || ring.indexOf(format) !== -1) continue
    ring.push(format)
  }
  return ring.length > 0 ? ring : ["HH:mm"]
}

// Next entry after `current`. An unknown current format (a hand-written one
// that is not in the ring) starts the walk at the top.
function nextClockFormat(ring, current) {
  if (!ring || ring.length === 0) return ""
  var index = ring.indexOf(String(current === undefined || current === null ? "" : current))
  return ring[(index + 1) % ring.length]
}

// Two-digit ISO week, substituted into a format's 'ww' token before Qt
// formats it -- Qt has no ISO week specifier of its own.
function isoWeekLiteral(year, month, day) {
  return pad2(isoWeek(year, month, day))
}

function pad2(value) {
  var n = Number(value)
  return (n < 10 ? "0" : "") + n
}

// English ordinals for the custom {ordinal} token; other locales use the day number.
function ordinalDay(value, locale) {
  var day = Number(value)
  if (locale && !/^en(?:[-_]|$)/i.test(String(locale)) && locale !== "C") return String(day)
  var remainder = day % 100
  var suffix = "th"

  if (remainder < 11 || remainder > 13) {
    if (day % 10 === 1) suffix = "st"
    else if (day % 10 === 2) suffix = "nd"
    else if (day % 10 === 3) suffix = "rd"
  }

  return String(day) + suffix
}

// Stable "yyyy-MM-dd" identity for a day, so a grid cell can be compared
// against today without dragging Date objects through bindings.
function dateKey(year, month, day) {
  return year + "-" + pad2(Number(month) + 1) + "-" + pad2(day)
}

function keyForDate(date) {
  return dateKey(date.getFullYear(), date.getMonth(), date.getDate())
}

// Date-only values use UTC solely for Gregorian arithmetic. They do not
// represent an event instant and must be read with the UTC getters.
function civilDate(key) {
  var match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(String(key || ""))
  if (!match) return null
  var year = Number(match[1]), month = Number(match[2]) - 1, day = Number(match[3])
  var date = new Date(Date.UTC(year, month, day))
  return date.getUTCFullYear() === year && date.getUTCMonth() === month
    && date.getUTCDate() === day ? date : null
}

function keyForCivilDate(date) {
  return dateKey(date.getUTCFullYear(), date.getUTCMonth(), date.getUTCDate())
}

// Turn either a Date or an ISO date/date-time into the local day identity used
// by the panel. Date-only strings are returned verbatim: parsing them with
// `new Date()` would reinterpret midnight as UTC and can move the day west of
// Greenwich.
function normalizedDateKey(value, fallback) {
  if (value === undefined || value === null || value === "") return String(fallback || "")

  var text = String(value)
  if (/^\d{4}-\d{2}-\d{2}$/.test(text)) return text

  var date = value instanceof Date ? value : new Date(value)
  if (!isNaN(date.getTime())) return keyForDate(date)
  return String(fallback || "")
}

function dateOnlyKey(value) {
  var text = String(value === undefined || value === null ? "" : value)
  var match = /^(\d{4}-\d{2}-\d{2})/.exec(text)
  return match ? match[1] : ""
}

// Resolve a local day boundary. A midnight can occur twice or not at all,
// and Qt's Date constructor can resolve a gap to the prior day.
function localDayBoundary(civilMidnight) {
  var before = new Date(civilMidnight - MS_PER_DAY).getTimezoneOffset()
  var after = new Date(civilMidnight + MS_PER_DAY).getTimezoneOffset()
  var low = civilMidnight + Math.min(before, after) * 60000
  var high = civilMidnight + Math.max(before, after) * 60000
  if (low === high) return { first: low }
  var first = new Date(low)
  var firstDay = Date.UTC(first.getFullYear(), first.getMonth(), first.getDate())
  var firstIsMidnight = firstDay === civilMidnight && first.getHours() === 0 && first.getMinutes() === 0
  var last = new Date(high)
  var lastDay = Date.UTC(last.getFullYear(), last.getMonth(), last.getDate())
  var lastIsMidnight = lastDay === civilMidnight && last.getHours() === 0 && last.getMinutes() === 0
  if (firstIsMidnight && lastIsMidnight && low < high) {
    // A rollback just after midnight can briefly enter this date, return to
    // the previous date, then enter it again at the second midnight.
    var left = low, right = high
    var offset = first.getTimezoneOffset()
    while (left < right) {
      var probe = Math.floor((left + right) / 2)
      if (new Date(probe).getTimezoneOffset() === offset) left = probe + 1
      else right = probe
    }
    return { first: low, last: high, rollback: left }
  }
  if (firstIsMidnight) return { first: low }
  if (lastIsMidnight) return { first: high }
  // Only a missing midnight reaches this path. This also leaves a skipped
  // civil day with identical start and end bounds rather than inventing time.
  while (low < high) {
    var middle = Math.floor((low + high) / 2)
    var date = new Date(middle)
    var day = Date.UTC(date.getFullYear(), date.getMonth(), date.getDate())
    if (day < civilMidnight) low = middle + 1
    else high = middle
  }
  return { first: low }
}

function localDayBounds(key) {
  var date = civilDate(key)
  if (!date) return null
  var midnight = date.getTime()
  var from = localDayBoundary(midnight)
  var to = localDayBoundary(midnight + MS_PER_DAY)
  var splitStart = from.rollback !== undefined && from.rollback < from.last
  var splitEnd = to.rollback !== undefined && to.rollback < to.last
  if (!splitStart && !splitEnd) return { start: from.first, end: to.first }

  var intervals = []
  var start = from.first
  if (splitStart) {
    intervals.push({ start: start, end: from.rollback })
    start = from.last
  }
  if (start < to.first) intervals.push({ start: start, end: to.first })
  if (splitEnd) intervals.push({ start: to.rollback, end: to.last })
  return { start: from.first, end: splitEnd ? to.last : to.first, intervals: intervals }
}

// Google all-day event ends are exclusive. Timed events are matched by local
// overlap so a meeting crossing midnight appears on both affected dates.
function itemOccursOnDate(item, key, dayBounds) {
  if (!item) return false
  var selected = String(key || "")
  if (selected === "") return false

  var type = String(item.type || "event").toLowerCase()
  if (type === "task") {
    var due = dateOnlyKey(item.dueDate || item.start)
    return due !== "" && due === selected
  }

  if (item.allDay === true) {
    var allDayStart = dateOnlyKey(item.start)
    var allDayEnd = dateOnlyKey(item.end)
    if (allDayStart === "") return false
    return selected >= allDayStart && (allDayEnd === "" ? selected === allDayStart : selected < allDayEnd)
  }

  var bounds = dayBounds || localDayBounds(selected)
  var start = new Date(item.start)
  if (!bounds || bounds.start >= bounds.end || isNaN(start.getTime())) return false
  var end = new Date(item.end)
  var endMs = isNaN(end.getTime()) ? start.getTime() + 1 : Math.max(start.getTime() + 1, end.getTime())
  if (bounds.intervals) {
    for (var i = 0; i < bounds.intervals.length; i++) {
      var interval = bounds.intervals[i]
      if (start.getTime() < interval.end && endMs > interval.start) return true
    }
    return false
  }
  return start.getTime() < bounds.end && endMs > bounds.start
}

function agendaRank(item) {
  if (String(item && item.type || "event").toLowerCase() === "task") return 2
  return item && item.allDay === true ? 0 : 1
}

function compareAgendaItems(left, right) {
  var rankDelta = agendaRank(left) - agendaRank(right)
  if (rankDelta !== 0) return rankDelta

  if (agendaRank(left) === 1) {
    var leftTime = new Date(left.start).getTime()
    var rightTime = new Date(right.start).getTime()
    if (isFinite(leftTime) && isFinite(rightTime) && leftTime !== rightTime) return leftTime - rightTime
  }

  var titleDelta = String(left.title || "").localeCompare(String(right.title || ""))
  if (titleDelta !== 0) return titleDelta
  return String(left.id || "").localeCompare(String(right.id || ""))
}

function shallowCopy(source) {
  var copy = {}
  for (var key in source) copy[key] = source[key]
  return copy
}

function appendUnique(target, value) {
  var text = String(value || "")
  if (text !== "" && target.indexOf(text) === -1) target.push(text)
}

function agendaItemsForDay(items, key, accountFilter, dayBounds) {
  var source = items && typeof items.length === "number" ? items : []
  var filter = String(accountFilter || "all")
  var bounds = dayBounds || localDayBounds(key)
  var out = []
  var sharedEvents = {}
  for (var i = 0; i < source.length; i++) {
    var item = source[i]
    if (!item) continue
    if (filter !== "all" && String(item.accountId || "") !== filter) continue
    if (!itemOccursOnDate(item, key, bounds)) continue

    var type = String(item.type || "event").toLowerCase()
    var sharedKey = type === "event" && item.icalUid
      ? String(item.icalUid) + "\n" + String(item.occurrenceStart || item.start || "")
      : ""
    if (sharedKey !== "" && sharedEvents[sharedKey] !== undefined) {
      var existing = out[sharedEvents[sharedKey]]
      appendUnique(existing.sourceLabels, item.accountLabel || item.calendarLabel)
      appendUnique(existing.sourceColors, item.color)
      continue
    }

    var display = shallowCopy(item)
    display.sourceLabels = []
    display.sourceColors = []
    appendUnique(display.sourceLabels, item.accountLabel || item.calendarLabel)
    appendUnique(display.sourceColors, item.color)
    out.push(display)
    if (sharedKey !== "") sharedEvents[sharedKey] = out.length - 1
  }
  out.sort(compareAgendaItems)
  return out
}

function dayDotColors(items, key, accountFilter, limit) {
  return agendaDotColors(agendaItemsForDay(items, key, accountFilter), limit)
}

function agendaDotColors(matching, limit) {
  var maximum = Math.max(0, Math.round(Number(limit === undefined ? 3 : limit)))
  var seen = {}
  var colors = []
  for (var i = 0; i < matching.length && colors.length < maximum; i++) {
    var itemColors = matching[i].sourceColors && matching[i].sourceColors.length > 0
      ? matching[i].sourceColors
      : [matching[i].color]
    for (var c = 0; c < itemColors.length && colors.length < maximum; c++) {
      var color = String(itemColors[c] || "")
      if (color === "" || seen[color]) continue
      seen[color] = true
      colors.push(color)
    }
  }
  return colors
}

// Bucket once per visible range, then prepare each day's display data once.
// UTC is used only to count calendar dates; timed events retain local dates.
function agendaIndex(items, firstKey, lastKey, accountFilter) {
  var first = Date.parse(firstKey + "T00:00:00Z")
  var last = Date.parse(lastKey + "T00:00:00Z")
  var days = []
  var result = {}
  for (var day = first; day <= last; day += MS_PER_DAY) {
    var key = keyForCivilDate(new Date(day))
    days.push({ key: key, bounds: localDayBounds(key), items: [] })
  }

  var filter = String(accountFilter || "all")
  var source = items || []
  for (var i = 0; i < source.length; i++) {
    var item = source[i]
    if (!item || (filter !== "all" && String(item.accountId || "") !== filter)) continue
    var startKey, endKey
    var exclusiveEnd = false
    if (String(item.type || "event").toLowerCase() === "task") {
      startKey = endKey = dateOnlyKey(item.dueDate || item.start)
    } else if (item.allDay === true) {
      startKey = dateOnlyKey(item.start)
      endKey = dateOnlyKey(item.end) || startKey
      exclusiveEnd = dateOnlyKey(item.end) !== ""
    } else {
      var start = new Date(item.start).getTime()
      if (!isFinite(start)) continue
      var end = new Date(item.end).getTime()
      end = isFinite(end) ? Math.max(start + 1, end) : start + 1
      // Local date can move backwards during a midnight clock rollback.
      // Compare real instants against the precomputed visible day boundaries.
      for (var d = 0; d < days.length; d++) {
        var target = days[d]
        if (start < target.bounds.end && end > target.bounds.start)
          target.items.push(item)
      }
      continue
    }
    var from = Math.max(0, (Date.parse(startKey + "T00:00:00Z") - first) / MS_PER_DAY)
    var through = Math.min(days.length - 1, (Date.parse(endKey + "T00:00:00Z") - first) / MS_PER_DAY - (exclusiveEnd ? 1 : 0))
    for (var index = from; index <= through; index++) days[index].items.push(item)
  }

  for (var date = 0; date < days.length; date++) {
    var entry = days[date]
    // Deduplicate after overlap, so copies with different spans stay correct.
    var matching = agendaItemsForDay(entry.items, entry.key, filter, entry.bounds)
    result[entry.key] = { items: matching, count: matching.length, colors: agendaDotColors(matching, 3) }
  }
  return result
}

function reminderOffsetLabel(value, translate) {
  var minutes = Number(value)
  if (!isFinite(minutes) || minutes < 0) return ""
  minutes = Math.round(minutes)
  if (minutes === 0) return typeof translate === "function" ? translate("At start") : "At start"
  var days = Math.floor(minutes / 1440)
  var hours = Math.floor((minutes % 1440) / 60)
  var remainder = minutes % 60
  var parts = []
  if (days > 0) parts.push(days + "d")
  if (hours > 0) parts.push(hours + "h")
  if (remainder > 0) parts.push(remainder + "m")
  var offset = parts.join(" ")
  return typeof translate === "function" ? translate("{offset} before", { offset: offset }) : offset + " before"
}

function reminderSummary(reminders, translate) {
  if (reminders === undefined || reminders === null || reminders === "") return ""
  var source = Array.isArray(reminders) ? reminders : [reminders]
  var labels = []
  for (var i = 0; i < source.length; i++) {
    var reminder = source[i]
    var label = ""
    if (typeof reminder === "number") label = reminderOffsetLabel(reminder, translate)
    else if (typeof reminder === "string") label = reminder
    else if (reminder && reminder.minutes !== undefined) label = reminderOffsetLabel(reminder.minutes, translate)
    else if (reminder && reminder.label !== undefined) label = String(reminder.label)
    if (label !== "" && labels.indexOf(label) === -1) labels.push(label)
  }
  return labels.join(", ")
}

function coerceWeekStart(value) {
  if (value === undefined || value === null) return null
  if (typeof value === "number")
    return isFinite(value) ? ((Math.round(value) % 7) + 7) % 7 : null

  var text = String(value).replace(/^\s+|\s+$/g, "").toLowerCase()
  if (text === "") return null

  for (var i = 0; i < WEEKDAY_NAMES.length; i++)
    if (WEEKDAY_NAMES[i] === text || WEEKDAY_NAMES[i].substr(0, 3) === text) return i

  var parsed = parseInt(text, 10)
  return isFinite(parsed) ? ((parsed % 7) + 7) % 7 : null
}

// Configured week start, falling back to the locale's own first day when
// the setting is missing or nonsense.
function normalizedWeekStart(value, fallback) {
  var configured = coerceWeekStart(value)
  if (configured !== null) return configured
  var fallbackStart = coerceWeekStart(fallback)
  return fallbackStart === null ? 1 : fallbackStart
}

function weekStartSettingName(index) {
  return WEEKDAY_NAMES[normalizedWeekStart(index, 1)]
}

// The toggle flips between the two conventions people actually switch
// between. A calendar configured to any other start (Saturday, say) is
// shown as-is and lands on Monday the first time it is toggled.
function toggledWeekStart(index) {
  return normalizedWeekStart(index, 1) === 1 ? 0 : 1
}

function weekdayOrder(weekStart) {
  var start = normalizedWeekStart(weekStart, 1)
  var out = []
  for (var i = 0; i < 7; i++) out.push((start + i) % 7)
  return out
}

// ISO-8601 week number: the week owning the Thursday of that date's
// Monday-based week. Mirrors the clock widget's 'ww' format token.
function isoWeek(year, month, day) {
  var date = new Date(Date.UTC(year, month, day))
  var weekday = date.getUTCDay() || 7
  date.setUTCDate(date.getUTCDate() + 4 - weekday)
  var yearStart = new Date(Date.UTC(date.getUTCFullYear(), 0, 1))
  return Math.ceil(((date.getTime() - yearStart.getTime()) / MS_PER_DAY + 1) / 7)
}

function dayOfYear(year, month, day) {
  return Math.round((Date.UTC(year, month, day) - Date.UTC(year, 0, 1)) / MS_PER_DAY) + 1
}

function daysInYear(year) {
  return dayOfYear(year, 11, 31)
}

// Share of the year already behind you: whole days completed over days in
// the year, so January 1 reads 0% and December 31 reads 100%.
function yearProgress(year, month, day) {
  var total = daysInYear(year)
  if (total <= 0) return 0
  return Math.max(0, Math.min(1, (dayOfYear(year, month, day) - 1) / total))
}

function yearProgressPercent(year, month, day) {
  return Math.round(yearProgress(year, month, day) * 100)
}

// Memento mori. The default span is a round number rather than anything from
// an actuarial table: the point of the bar is the reminder, not the
// arithmetic, and whoever wants a different number can say so.
var DEFAULT_LIFE_EXPECTANCY = 90

// A birth year rather than an age, so the bar keeps counting on its own
// instead of going stale the moment it is entered. 0 means "not set", which
// is also what a blank, malformed, future, or implausibly distant year means.
function parseBirthYear(value, currentYear) {
  var now = Math.round(Number(currentYear))
  if (!isFinite(now)) return 0
  var text = String(value === undefined || value === null ? "" : value).replace(/^\s+|\s+$/g, "")
  if (!/^\d{4}$/.test(text)) return 0
  var year = parseInt(text, 10)
  if (!isFinite(year) || year > now || year < now - 120) return 0
  return year
}

// Whole years, the way people say their age: born in 1979 makes you 47 for
// all of 2026, whichever side of your birthday today falls.
function ageFromBirthYear(birthYear, currentYear) {
  var born = parseBirthYear(birthYear, currentYear)
  if (born <= 0) return 0
  return Math.round(Number(currentYear)) - born
}

// 0 means "not set", which is also what a blank, negative, fractional, or
// absurd entry means — the life bar simply stays hidden.
function parseAge(value) {
  var text = String(value === undefined || value === null ? "" : value).replace(/^\s+|\s+$/g, "")
  if (!/^\d+$/.test(text)) return 0
  var years = parseInt(text, 10)
  if (!isFinite(years) || years <= 0 || years > 120) return 0
  return years
}

// Unset or nonsense falls back to the default rather than to zero, so the
// bar always has something to measure against.
function parseLifeExpectancy(value) {
  var text = String(value === undefined || value === null ? "" : value).replace(/^\s+|\s+$/g, "")
  if (!/^\d+$/.test(text)) return DEFAULT_LIFE_EXPECTANCY
  var years = parseInt(text, 10)
  if (!isFinite(years) || years <= 0 || years > 150) return DEFAULT_LIFE_EXPECTANCY
  return years
}

function lifeProgress(age, expectancy) {
  var years = parseAge(age)
  var span = parseLifeExpectancy(expectancy)
  if (years <= 0 || span <= 0) return 0
  return Math.max(0, Math.min(1, years / span))
}

function lifeProgressPercent(age, expectancy) {
  return Math.round(lifeProgress(age, expectancy) * 100)
}

// Always six rows of seven days. A fixed grid keeps the popup exactly the
// same height in every month, so stepping through the year never makes the
// panel jump under the pointer.
function monthGrid(year, month, weekStart, todayKey) {
  var start = normalizedWeekStart(weekStart, 1)
  var leading = (new Date(Date.UTC(year, month, 1)).getUTCDay() - start + 7) % 7
  var cursor = new Date(Date.UTC(year, month, 1 - leading))
  var today = String(todayKey || "")
  var weeks = []

  for (var w = 0; w < 6; w++) {
    var days = []
    var thursday = null
    for (var d = 0; d < 7; d++) {
      var cellYear = cursor.getUTCFullYear()
      var cellMonth = cursor.getUTCMonth()
      var cellDay = cursor.getUTCDate()
      var weekday = cursor.getUTCDay()
      var key = dateKey(cellYear, cellMonth, cellDay)
      if (weekday === 4) thursday = { year: cellYear, month: cellMonth, day: cellDay }
      days.push({
        key: key,
        year: cellYear,
        month: cellMonth,
        day: cellDay,
        weekday: weekday,
        inMonth: cellMonth === month && cellYear === year,
        weekend: weekday === 0 || weekday === 6,
        today: key === today
      })
      cursor.setUTCDate(cursor.getUTCDate() + 1)
    }
    // Number every row by the ISO week owning its Thursday. That is the
    // definition itself for Monday-start weeks, and the only answer that
    // stays stable for the other starts, where a row straddles two ISO
    // weeks but shares all of Monday through Thursday with one of them.
    var anchor = thursday || days[0]
    weeks.push({
      week: isoWeek(anchor.year, anchor.month, anchor.day),
      days: days
    })
  }
  return weeks
}

function stepMonth(year, month, delta) {
  var target = new Date(Date.UTC(year, Number(month) + Number(delta), 1))
  return { year: target.getUTCFullYear(), month: target.getUTCMonth() }
}

if (typeof module !== "undefined") {
  module.exports = {
    ordinalDay: ordinalDay,
    dateKey: dateKey,
    keyForDate: keyForDate,
    civilDate: civilDate,
    keyForCivilDate: keyForCivilDate,
    localDayBounds: localDayBounds,
    normalizedDateKey: normalizedDateKey,
    itemOccursOnDate: itemOccursOnDate,
    agendaItemsForDay: agendaItemsForDay,
    dayDotColors: dayDotColors,
    agendaIndex: agendaIndex,
    reminderOffsetLabel: reminderOffsetLabel,
    reminderSummary: reminderSummary,
    normalizedWeekStart: normalizedWeekStart,
    weekStartSettingName: weekStartSettingName,
    toggledWeekStart: toggledWeekStart,
    weekdayOrder: weekdayOrder,
    isoWeek: isoWeek,
    dayOfYear: dayOfYear,
    daysInYear: daysInYear,
    yearProgress: yearProgress,
    yearProgressPercent: yearProgressPercent,
    parseAge: parseAge,
    parseBirthYear: parseBirthYear,
    ageFromBirthYear: ageFromBirthYear,
    parseLifeExpectancy: parseLifeExpectancy,
    lifeProgress: lifeProgress,
    lifeProgressPercent: lifeProgressPercent,
    monthGrid: monthGrid,
    stepMonth: stepMonth,
    clockFormats: clockFormats,
    clockFormatRing: clockFormatRing,
    nextClockFormat: nextClockFormat,
    isoWeekLiteral: isoWeekLiteral
  }
}
