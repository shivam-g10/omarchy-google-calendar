// Run: node tests/agenda-benchmark.js [--check]
// Node-only synthetic comparison; it does not measure Qt, rendering, or backend I/O.
const assert = require('node:assert/strict')
const { execFileSync } = require('node:child_process')
const { readFileSync } = require('node:fs')
const { cpus } = require('node:os')
const { resolve } = require('node:path')
const { performance } = require('node:perf_hooks')
const root = resolve(__dirname, '..')
const references = { main: '60ba50c', pr: '4122316', revised: 'working-tree' }
const models = Object.fromEntries(Object.entries(references).map(([name, ref]) => {
  const source = ref === 'working-tree' ? readFileSync(resolve(root, 'Model.js'), 'utf8')
    : execFileSync('git', ['show', `${ref}:Model.js`], { cwd: root, encoding: 'utf8' })
  const module = { exports: {} }
  new Function('module', source)(module)
  return [name, module.exports]
}))
const samples = Number(process.env.AGENDA_BENCH_SAMPLES || 40)
const warmups = Number(process.env.AGENDA_BENCH_WARMUPS || 5)
const checkOnly = process.argv.includes('--check')
const cases = [
  ...[75, 1000, 5000].map(count => ({ name: `mixed-${count}`, count, zone: 'Asia/Kolkata', year: 2026, month: 9 })),
  { name: 'midnight-gap-1000', count: 1000, zone: 'America/Santiago', year: 2026, month: 8 },
  { name: 'midnight-rollback-400', count: 400, zone: 'America/St_Johns', year: 2009, month: 9 },
]

function fixture(scenario, keys) {
  const next = key => new Date(Date.parse(key + 'T00:00:00Z') + 86400000).toISOString().slice(0, 10)
  return Array.from({ length: scenario.count }, (_, i) => {
    // Adjacent shared copies, multiple accounts, tasks, all-day and overnight events.
    const slot = i % 10 < 2 ? i - i % 2 : i
    const day = keys[(slot * 17) % keys.length]
    const item = { id: String(i), type: 'event', accountId: String(i % 3),
      title: `Item ${slot}`, start: day + 'T10:00:00', end: day + 'T11:00:00',
      color: ['red', 'blue', 'green'][i % 3] }
    if (i % 10 < 2) item.icalUid = `shared-${slot}`
    if (i % 10 === 2) Object.assign(item, { type: 'task', dueDate: day })
    if (i % 10 === 3) Object.assign(item, { allDay: true, start: day, end: next(next(day)) })
    if (i % 10 === 4) Object.assign(item, { start: day + 'T23:30:00', end: next(day) + 'T01:00:00' })
    if (scenario.zone === 'America/St_Johns' && i % 5 === 0)
      Object.assign(item, { start: '2009-10-31T23:10:00-03:30', end: '2009-10-31T23:20:00-03:30' })
    return item
  })
}

function prepare(model, items, keys, filter) {
  if (model.agendaIndex) return model.agendaIndex(items, keys[0], keys[keys.length - 1], filter)
  // Original UI independently prepared the count and dots for each of 42 cells.
  return Object.fromEntries(keys.map(key => {
    const itemsForDay = model.agendaItemsForDay(items, key, filter)
    return [key, { items: itemsForDay, count: itemsForDay.length,
      colors: model.dayDotColors(items, key, filter, 3) }]
  }))
}

console.log(JSON.stringify({ node: process.version, cpu: cpus()[0].model, references,
  samples: checkOnly ? 0 : samples, warmups: checkOnly ? 0 : warmups, units: 'ms', fixture: 'synthetic, 42 days' }))
for (const scenario of cases) {
  process.env.TZ = scenario.zone
  const keys = models.revised.monthGrid(scenario.year, scenario.month, 1, '').flatMap(w => w.days.map(d => d.key))
  const items = fixture(scenario, keys)
  let matchesMain = true
  for (const filter of ['all', '1']) {
    const current = prepare(models.revised, items, keys, filter)
    assert.deepEqual(current, prepare(models.pr, items, keys, filter), `${scenario.name}: PR equivalence`)
    const baseline = prepare(models.main, items, keys, filter)
    if (scenario.zone === 'America/St_Johns') {
      try { assert.deepEqual(current, baseline) } catch { matchesMain = false }
    } else assert.deepEqual(current, baseline, `${scenario.name}: main equivalence`)
  }
  const result = { fixture: scenario.name, timezone: scenario.zone, items: items.length, matchesMain, matchesPR: true }
  if (!matchesMain) result.note = 'Main has the known midnight-rollback correctness bug.'
  if (!checkOnly) {
    const names = Object.keys(models)
    const durations = Object.fromEntries(names.map(name => [name, []]))
    for (let i = 0; i < warmups; i++)
      for (const name of names) prepare(models[name], items, keys, 'all')
    // Rotate execution order so every version shares early and late positions.
    for (let sample = 0; sample < samples; sample++) {
      for (let offset = 0; offset < names.length; offset++) {
        const name = names[(sample + offset) % names.length]
        const start = performance.now()
        prepare(models[name], items, keys, 'all')
        durations[name].push(performance.now() - start)
      }
    }
    for (const name of names) {
      const values = durations[name].sort((a, b) => a - b)
      result[name] = { p50: +values[Math.ceil(values.length * .5) - 1].toFixed(3),
        p95: +values[Math.ceil(values.length * .95) - 1].toFixed(3) }
    }
  }
  console.log(JSON.stringify(result))
}
