const assert = require('node:assert/strict')
const { test } = require('node:test')
const Model = require('../Model.js')

const event = (id, start, end, extra = {}) => ({
  id, type: 'event', accountId: 'a', title: id, start, end, color: '#aabbcc', ...extra,
})
const items = [
  event('timed', '2026-10-06T10:00:00', '2026-10-06T11:00:00'),
  event('overnight', '2026-10-06T23:00:00', '2026-10-07T01:00:00'),
  event('midnight', '2026-10-06T23:00:00', '2026-10-07T00:00:00'),
  event('all-day', '2026-10-05', '2026-10-07', { allDay: true }),
  event('no-end', '2026-10-06T12:00:00', undefined),
  event('invalid-end', '2026-10-06T13:00:00', 'invalid'),
  event('invalid-start', 'invalid', 'invalid'),
  event('reversed-end', '2026-10-06T14:00:00', '2026-10-06T13:00:00'),
  event('long', '2020-01-01', '2030-01-01', { allDay: true }),
  event('single-all-day', '2026-10-06', undefined, { allDay: true }),
  { id: 'task', type: 'task', accountId: 'a', dueDate: '2026-10-06', title: 'Task' },
  event('shared-a', '2026-10-06T15:00:00', '2026-10-06T16:00:00', {
    icalUid: 'shared', occurrenceStart: '2026-10-06T15:00:00', accountLabel: 'A',
  }),
  event('shared-b', '2026-10-06T15:00:00', '2026-10-07T16:00:00', {
    icalUid: 'shared', occurrenceStart: '2026-10-06T15:00:00',
    accountId: 'b', accountLabel: 'B', color: '#ddeeff',
  }),
  event('next-occurrence', '2026-10-07T15:00:00', '2026-10-07T16:00:00', {
    icalUid: 'shared', occurrenceStart: '2026-10-07T15:00:00',
  }),
]

test('index preserves agenda ordering, counts, colors and account filtering', () => {
  const original = structuredClone(items)
  for (const filter of ['all', 'a', 'b', 'missing']) {
    const index = Model.agendaIndex(items, '2026-10-01', '2026-10-31', filter)
    assert.equal(Object.keys(index).length, 31)
    for (const [key, day] of Object.entries(index)) {
      assert.deepEqual(day.items, Model.agendaItemsForDay(items, key, filter), key)
      assert.deepEqual(day.colors, Model.dayDotColors(items, key, filter, 3), key)
      assert.equal(day.count, day.items.length)
    }
  }
  assert.deepEqual(items, original)
})

test('exclusive ends and shared copies retain per-day meaning', () => {
  const index = Model.agendaIndex(items, '2026-10-06', '2026-10-07', 'all')
  const next = index['2026-10-07'].items
  assert(!next.some(item => ['midnight', 'all-day', 'single-all-day', 'task'].includes(item.id)))
  assert(next.some(item => item.id === 'overnight'))
  assert(next.some(item => item.id === 'next-occurrence'))
  assert.deepEqual(index['2026-10-06'].items.find(item => item.id === 'shared-a').sourceLabels, ['A', 'B'])
  assert.deepEqual(next.find(item => item.id === 'shared-b').sourceLabels, ['B'])
})

test('empty ranges, changed ranges and updated items do not retain old results', () => {
  assert.deepEqual(Model.agendaIndex(items, '2026-10-07', '2026-10-06', 'all'), {})
  assert.deepEqual(Model.agendaIndex([], '2026-10-06', '2026-10-06', 'all'), {
    '2026-10-06': { items: [], count: 0, colors: [] },
  })
  assert.deepEqual(Object.keys(Model.agendaIndex(items, '2026-11-01', '2026-11-02', 'all')),
    ['2026-11-01', '2026-11-02'])
})
