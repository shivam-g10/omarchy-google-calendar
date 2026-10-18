"""Measure the real Model.js in Qt's JS engine without starting the shell/backend.

Run: python tests/model_benchmark.py > model-benchmark.jsonl
The percentiles describe per-operation averages of calibrated batches, not
individual-call latency. Date.now() has 1 ms resolution in the QML harness.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SCENARIOS = [
    {"timezone": "UTC", "year": 2026, "month": 9},
    {"timezone": "Asia/Kolkata", "year": 2026, "month": 9},
    {"timezone": "America/Santiago", "year": 2026, "month": 9},
    {"timezone": "America/St_Johns", "year": 2009, "month": 10},
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--samples", type=int, default=40)
    parser.add_argument("--warmups", type=int, default=5)
    parser.add_argument("--target-ms", type=int, default=20)
    args = parser.parse_args()
    if args.samples < 1 or args.warmups < 0 or args.target_ms < 1:
        parser.error("samples and target-ms must be positive; warmups cannot be negative")
    qml = shutil.which("qml6") or shutil.which("qml")
    if not qml:
        parser.error("Qt QML runner required (qml6 or qml)")
    env = os.environ.copy()
    env.update(QT_QPA_PLATFORM="offscreen", QT_FORCE_STDERR_LOGGING="1")
    version = subprocess.run([qml, "-a", "core", "--version"], env=env,
                             capture_output=True, text=True, check=True)
    source = (ROOT / "Model.js").read_bytes()
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    committed = subprocess.check_output(["git", "show", "HEAD:Model.js"], cwd=ROOT)
    print(json.dumps({"kind": "metadata", "runner": qml,
        "qtVersion": (version.stdout + version.stderr).strip(), "head": revision,
        "modelSha256": hashlib.sha256(source).hexdigest(), "modelMatchesHead": source == committed,
        "samples": args.samples, "warmups": args.warmups, "targetBatchMs": args.target_ms,
        "timer": "Date.now", "timerResolutionMs": 1,
        "distribution": "per-operation averages of timed batches; not individual-call latency",
        "scope": "Qt JavaScript only; excludes rendering, shell, backend and network"}), flush=True)
    template = (ROOT / "tests/model_benchmark.qml").read_text()
    with tempfile.TemporaryDirectory(prefix="calendar-model-benchmark-") as directory:
        path = Path(directory) / "benchmark.qml"
        for scenario in SCENARIOS:
            options = dict(scenario, samples=args.samples, warmups=args.warmups, targetMs=args.target_ms)
            content = template.replace('import "../Model.js" as Model',
                f'import {json.dumps((ROOT / "Model.js").as_uri())} as Model')
            content = content.replace("property var options: ({})", f"property var options: ({json.dumps(options)})")
            path.write_text(content)
            result = subprocess.run([qml, "-a", "core", str(path)], env=dict(env, TZ=scenario["timezone"]),
                                    capture_output=True, text=True, timeout=120)
            output = result.stdout + result.stderr
            if result.returncode:
                raise RuntimeError(output)
            rows = [json.loads(line.split("MODEL_BENCH ", 1)[1]) for line in output.splitlines()
                    if "MODEL_BENCH " in line]
            if len(rows) != 3:
                raise RuntimeError(f"Expected three benchmark results:\n{output}")
            for row in rows:
                print(json.dumps(row), flush=True)


if __name__ == "__main__":
    main()
