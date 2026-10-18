"""Measure production locale helpers in Qt, without the shell or calendar daemon."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
QML = shutil.which("qml6") or shutil.which("qml") or "/usr/lib/qt6/bin/qml"


def function(source, name):
    return re.search(rf"^  function {name}\(.*?^  }}", source, re.M | re.S).group(0)


def main():
    panel = (ROOT / "Panel.qml").read_text()
    bar = (ROOT / "BarWidget.qml").read_text()
    imports = "import QtQml\n" + "\n".join(
        f'import {json.dumps((ROOT / filename).as_uri())} as {name}'
        for filename, name in (("Model.js", "Model"), ("I18n.js", "I18n")))
    helpers = "\n".join(function(panel, name) for name in
                        ("tr", "selectedDateLabel", "agendaSource", "agendaMeta"))
    helpers += "\n" + function(bar, "formatted")
    source = (ROOT / "tests/locale-benchmark.qml").read_text()
    source = source.replace("// IMPORTS", imports).replace("// HELPERS", helpers)
    with tempfile.TemporaryDirectory(prefix="calendar-locale-benchmark-") as directory:
        path = Path(directory) / "benchmark.qml"
        path.write_text(source)
        env = os.environ.copy()
        env.update(QT_QPA_PLATFORM="offscreen", QT_FORCE_STDERR_LOGGING="1", TZ="UTC")
        result = subprocess.run([QML, "-a", "core", str(path)], env=env,
                                capture_output=True, text=True, timeout=60)
        output = result.stdout + result.stderr
        if result.returncode or "LOCALE_BENCHMARK_DONE" not in output:
            raise SystemExit(output)
        for line in output.splitlines():
            if "LOCALE_BENCHMARK " in line:
                print(line.split("LOCALE_BENCHMARK ", 1)[1])


if __name__ == "__main__":
    main()
