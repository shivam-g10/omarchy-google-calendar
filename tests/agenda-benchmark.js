// Run with: node tests/agenda-benchmark.js
// Both paths use the current date handling; this isolates the indexing cost.
const { performance } = require('node:perf_hooks')
const Model = require('../Model.js')
const keys = Model.monthGrid(2026, 9, 1, '').flatMap(week => week.days.map(day => day.key))

function median(work) {
  for (let i = 0; i < 20; i++) work()
  const samples = []
  for (let i = 0; i < 100; i++) {
    const start = performance.now()
    work()
    samples.push(performance.now() - start)
  }
  return samples.sort((a, b) => a - b)[50].toFixed(2)
}

for (const count of [75, 1000]) {
  const items = Array.from({ length: count }, (_, i) => ({
    id: String(i), type: 'event', accountId: String(i % 3), title: `Event ${i}`,
    start: keys[i % keys.length] + 'T10:00:00',
    end: keys[i % keys.length] + 'T11:00:00', color: ['red', 'blue', 'green'][i % 3],
  }))
  const before = median(() => keys.forEach(key => {
    Model.dayDotColors(items, key, 'all', 3)
    Model.agendaItemsForDay(items, key, 'all').length
  }))
  const after = median(() => Model.agendaIndex(items, keys[0], keys[keys.length - 1], 'all'))
  console.log(`${count} items: ${before} ms repeated scans, ${after} ms indexed (median; JavaScript only)`)
}
