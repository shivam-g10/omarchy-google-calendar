"""Exercise installer ownership in isolated homes with mocked host commands."""
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class InstallerTests(unittest.TestCase):
    def exercise(self, custom_home, extra_file):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            home = base / "home"
            bins = base / "bin"
            runtime = base / "runtime"
            for path in (home, bins, runtime):
                path.mkdir()
            env = os.environ.copy()
            env.update(HOME=str(home), XDG_CONFIG_HOME=str(home / ".config"),
                       XDG_CACHE_HOME=str(home / ".cache"),
                       XDG_RUNTIME_DIR=str(runtime), PATH=f"{bins}:{env['PATH']}")
            env.pop("CODEX_HOME", None)
            codex_home = base / "custom-codex" if custom_home else home / ".codex"
            if custom_home:
                env["CODEX_HOME"] = str(codex_home)
            for name in ("pkg-config", "secret-tool", "systemctl", "xdg-open"):
                target = bins / name
                target.write_text("#!/bin/sh\nexit 0\n")
                target.chmod(0o755)
            cargo = bins / "cargo"
            cargo.write_text('''#!/bin/bash
set -eu
while [[ "$1" != "--target-dir" ]]; do shift; done
mkdir -p "$2/release"
printf '#!/bin/sh\\nexit 0\\n' > "$2/release/omarchy-calendar"
''')
            cargo.chmod(0o755)
            listener = socket.socket(socket.AF_UNIX)
            self.addCleanup(listener.close)
            listener.bind(str(runtime / "omarchy-calendar.sock"))
            skill = codex_home / "skills/omarchy-calendar/SKILL.md"
            unrelated = codex_home / "skills/unrelated/SKILL.md"
            unrelated.parent.mkdir(parents=True)
            unrelated.write_text("keep")
            def run(script):
                subprocess.run(["bash", str(ROOT / "scripts" / script)], env=env,
                               check=True, capture_output=True, text=True, timeout=10)
            run("install-backend")
            self.assertEqual(skill.read_bytes(), (ROOT / "skills/omarchy-calendar/SKILL.md").read_bytes())
            skill.write_text("old version")
            run("install-backend")
            self.assertEqual(skill.read_bytes(), (ROOT / "skills/omarchy-calendar/SKILL.md").read_bytes())
            if extra_file:
                (skill.parent / "personal.txt").write_text("keep")
            run("uninstall-backend")
            run("uninstall-backend")
            self.assertFalse(skill.exists())
            self.assertEqual(skill.parent.exists(), extra_file)
            self.assertEqual(unrelated.read_text(), "keep")
            self.assertFalse((home / ".local/bin/omarchy-calendar").exists())
            self.assertFalse((home / ".config/systemd/user/omarchy-calendar.service").exists())
            listener.close()

    def test_default_skill_install_update_and_removal(self):
        self.exercise(False, False)

    def test_custom_codex_home_preserves_user_files(self):
        self.exercise(True, True)


if __name__ == "__main__":
    unittest.main()
