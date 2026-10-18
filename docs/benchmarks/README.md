# Calendar PR measurements

Measured on October 18, 2026 on an AMD Ryzen 7 9700X (8 cores / 16 threads),
Linux 7.2.5, performance CPU governor, Qt 6.11.2, Quickshell 0.3.1, Omarchy 4.0.4,
and Node 26.12.0. Benchmarks ran sequentially on the same desktop machine, with
synthetic data and isolated processes. These are local measurements, not a
cross-machine performance guarantee. No real accounts, credentials, or events
were used. No new production dependencies were added.

`main` means `60ba50c`; `previous PR` means `4122316`. The revised source hashes
are recorded in `environment.json`. Raw results live beside this report.

## Agenda preparation

The fixture includes shared copies, account filters, tasks, all-day events, and
overnight events across 42 dates. Complete outputs are checked against the
previous PR before timing. Main matches except for its known midnight rollback
bug. Execution order rotates between revisions; each has five warmups and 40
samples. These Node timings exclude rendering and backend work.

| Fixture | Main p50 / p95, ms | Previous PR p50 / p95, ms | Revised p50 / p95, ms |
| --- | ---: | ---: | ---: |
| 75 items | 2.341 / 2.429 | 0.201 / 0.230 | 0.093 / 0.123 |
| 1,000 items | 34.786 / 37.447 | 2.675 / 2.829 | 1.305 / 1.612 |
| 5,000 items | 177.760 / 181.062 | 15.213 / 15.720 | 8.397 / 8.689 |
| Santiago midnight gap, 1,000 | 51.425 / 52.824 | 3.138 / 3.212 | 1.806 / 1.869 |
| St. John's midnight rollback, 400 | Incorrect output | 1.047 / 1.068 | 0.600 / 0.626 |

The additional improvement comes from using an array of day records instead of
timestamp-keyed object lookups and computing each day's timezone bounds once.
The DST boundary algorithm and per-day deduplication semantics are unchanged.

## Actual Qt model work

The production Model.js also runs in Qt 6.11.2's JavaScript engine. Each operation
has five warmup batches and 40 measured batches, calibrated to at least 20 ms.
Percentiles below describe per-operation batch averages. The 1,000-event batches
contain two calls, so their effective resolution is 0.5 ms. These fixtures are
timed events; the Node comparison above uses a mixed fixture.

| Zone | Grid p50 / p95, ms | 42 day bounds p50 / p95, ms | 1,000-event index p50 / p95, ms |
| --- | ---: | ---: | ---: |
| UTC | 0.053 / 0.055 | 0.098 / 0.102 | 9.5 / 9.5 |
| Kolkata | 0.053 / 0.055 | 0.113 / 0.125 | 11 / 11 |
| Santiago, September 2026 | 0.053 / 0.055 | 0.141 / 0.145 | 9.5 / 10 |
| St. John's, October 2009 | 0.053 / 0.055 | 0.117 / 0.125 | 10 / 10 |

## Rendered panel

This runs the complete production panel with installed Omarchy controls and
synthetic accounts/events. Only the compositor-facing window is replaced by an
offscreen Qt Window. The software renderer reports action-to-frameSwapped time,
including bindings, layout, and rendering. It does not measure GPU submission,
Wayland presentation, or input-to-photon latency. A 1-pixel marker guarantees a
frame even if an action leaves visible content unchanged.

Each action has five warmups and 30 measured frames. A 30 ms pause separates
actions. The date is fixed to October 18, 2026 in UTC; events span 28 dates.
Numbers are p50 / p95 in milliseconds.

| Items / action | Main | Previous PR | Revised |
| --- | ---: | ---: | ---: |
| 75 / items update | 21 / 22 | 8 / 9 | 7 / 8 |
| 75 / select date | 6 / 7 | 6 / 7 | 6 / 7 |
| 75 / change month | 26 / 28 | 11 / 13 | 9 / 11 |
| 1,000 / items update | 263 / 268 | 33 / 36 | 21 / 23 |
| 1,000 / select date | 11 / 14 | 8 / 11 | 8 / 11 |
| 1,000 / change month | 268 / 275 | 56 / 58 | 32 / 35 |
| Either / English–Spanish switch | Unsupported | 6 / 7 | 6 / 7 |

The 1,000-item update still exceeds a 16.7 ms frame budget in this software-rendered
fixture. The improvement is measured; a blanket 60 fps claim would be unsupported.

## Widget startup and idle memory

The complete production bar widget and panel run with 1,000 synthetic events and
the same installed controls/offscreen window. Three fresh processes per revision
measure object construction and first/repeated opening. These construction times
exclude process startup and initial QML compilation. They use a 1 ms clock, so a
reported 0 ms means below that resolution.

| Metric | Previous PR | Revised |
| --- | ---: | ---: |
| Widget construction, median (range) | 88 ms (87–89) | 12 ms (12–12) |
| First open through rendered frame, median (range) | 4 ms (4–5) | 37 ms (37–37) |
| Reopen through rendered frame, median (range) | 1 ms (1–1) | 1 ms (0–1) |
| Unopened process PSS / RSS, median | 99.82 / 152.64 MiB | 90.43 / 142.83 MiB |
| Open process PSS / RSS, median | 138.92 / 191.89 MiB | 135.58 / 188.45 MiB |
| Closed after use, process PSS / RSS, median | 138.92 / 191.89 MiB | 135.58 / 188.45 MiB |

The panel is created on the first open or week-start action and retained for
reuse. This trades a 33 ms increase in first-open latency for 76 ms less widget
construction work and about 9.4 MiB lower unopened PSS in this standalone fixture.
Closing does not unload the panel. These totals include the test shell, Qt, fonts,
and renderer; they are not incremental memory costs inside an already running
desktop shell. RSS includes shared pages; PSS apportions them.

Each state in the first process per revision has a 10-second idle sample; no CPU
ticks were observed. At the 10 ms Linux tick resolution, that does not prove zero
CPU use (roughly 0.1% of one core is the measurement granularity). The other two
processes use 0.1-second resource samples only to repeat construction/memory
observations; their CPU samples are not used for a conclusion.

## Real client and daemon resources

The release Rust daemon and production CalendarService/BarWidget run in private
HOME/XDG directories with networking disabled. Presentation is mocked in this
fixture; the rendered widget is measured separately above. The populated case
seeds a temporary SQLite database with one synthetic account/calendar and 1,000
events, then asserts that every active client receives all 1,000 items. A private
`secret-tool` stub prevents access to the real keyring. No Google requests occur.

One final run per path has a three-second warmup followed by five memory snapshots
over 20 seconds. Values are mean combined shell-plus-daemon MiB. All standalone
shell costs are included; subtracting these numbers from a live desktop's usage
would not establish the plugin's incremental cost there.

| Cache / path | PSS | RSS | Daemon clients |
| --- | ---: | ---: | ---: |
| Empty / bare shell, no daemon | 55.81 | 103.82 | 0 |
| Empty / shell + daemon, no client | 58.05 | 109.54 | 0 |
| Empty / shared client + widget | 64.32 | 116.50 | 1 |
| Empty / one fallback widget | 64.49 | 116.63 | 1 |
| Empty / three fallback widgets | 64.40 | 116.34 | 1 |
| Empty / shared + fallback | 64.64 | 116.92 | 2 |
| Empty / switch back to shared | 64.67 | 116.97 | 2 |
| 1,000 events / shared client + widget | 71.14 | 123.53 | 1 |
| 1,000 events / shared + fallback | 75.98 | 128.22 | 2 |
| 1,000 events / switch back to shared | 76.00 | 128.27 | 2 |

The additional client costs about 0.31 MiB PSS empty and 4.84 MiB with 1,000 items
in this run. It adds one persistent backend client thread and three descriptors
across the processes. The connection/cache remains after switching back to the
built-in bar. Three replacement widgets share one fallback; they do not create
three sockets or event caches. Small cross-process memory differences should not
be interpreted as savings, such as three widgets appearing smaller than one.

Observed connected idle CPU was 0–0.01 CPU seconds per 20 seconds, or 0–0.05% of one
core. The Linux tick resolution is 10 ms. This does not establish zero background
work, active Google synchronization cost, or behavior across a full sync interval.
The shared service contract remains unchanged; eliminating its duplicate cache
after both bar types have been used would require a separate lifecycle change.

## Locale helpers

Qt core tests exercise the production translation, selected-date label, agenda
metadata, and custom bar-format functions. Nine locale variants include English
US/GB, Spanish, French, German, Portuguese, Hindi, Simplified Chinese, and Japanese.
Each path has 500 warmup calls and 11 measured batches of 3,000 or 10,000 calls.
`Date.now()` has 1 ms resolution; reported microseconds are batch averages.

| Path | Median across locales, microseconds/call |
| --- | ---: |
| Translation with substitution | 1.3–1.4 |
| Selected-date label | 3.33–3.67 |
| Reminder and agenda metadata | 4.33–4.67 |
| Custom clock format | 3.67 |

All seven translated catalogs contain the same 60 source keys and matching
placeholders. English uses the source strings. Unsupported languages fall back
to English UI text while Qt still formats dates in the requested locale.

The complete panel was rendered and inspected in English GB, Spanish, French,
German, Portuguese, Hindi, Simplified Chinese, and Japanese. Example synthetic
renders: [English GB](en_GB.png), [Hindi](hi_IN.png), [Japanese](ja_JP.png).
The translations have automated coverage and visual checks, not native-speaker review.

## Validation and code size

- 55 Rust tests passed; one internal peer-server helper is intentionally ignored.
- Seven Python tests passed, including actual Qt date regressions in seven timezones,
  installer isolation, actual bar locale formatting, lazy loading, and service lifecycle.
- Four Node agenda tests and complete-output benchmark equivalence checks passed.
- Nine Qt locale test results passed, including catalog/placeholder parity and `en_GB`.
- Full rendered widget settings propagation, QML parsing, and diff checks passed.

Compared with the previous PR, Model.js has the same line count and BarWidget.qml
has eight fewer lines. Declarative bindings replace manual panel injection; the
date algorithm is retained. The size increase is translation data, regression
tests, benchmark harnesses, and this evidence. No backend or dependency changes
were needed. The PR is not installed on the live desktop or merged.

## Reproduce

Run from the repository root. The render/resource harnesses require Linux,
Quickshell, and installed Omarchy controls at `/usr/share/omarchy/shell`.
The service harness also requires unprivileged user/network namespaces.

```bash
node tests/agenda-benchmark.js
python3 tests/model_benchmark.py
python3 tests/locale_benchmark.py
python3 tests/panel-benchmark.py --ref 60ba50c --items 1000
python3 tests/panel-benchmark.py --ref 4122316 --items 1000
python3 tests/panel-benchmark.py --items 1000
python3 tests/widget-benchmark.py --ref 4122316 --items 1000
python3 tests/widget-benchmark.py --items 1000
cargo build --locked --release --manifest-path calendar-rs/Cargo.toml --target-dir /tmp/calendar-bench-target
python3 tests/service-benchmark.py --binary /tmp/calendar-bench-target/release/omarchy-calendar --seconds 20
python3 tests/service-benchmark.py --binary /tmp/calendar-bench-target/release/omarchy-calendar --items 1000 --case shared_widget --case shared_plus_fallback --seconds 20
```

Run one benchmark at a time. The scripts use temporary HOME/XDG directories and
clean up their processes; they do not install the plugin or restart the user's
calendar service. The service benchmark disables network access in its process
namespace. The rendering benchmarks use synthetic state instead of a daemon.
