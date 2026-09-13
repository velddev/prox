"""Exercise installation using isolated home directories; restore Windows user PATH."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

EXE = Path(sys.argv[1]).resolve()


def run(exe, env, *args):
    return subprocess.run([str(exe), *args], env=env, capture_output=True,
                          text=True, encoding="utf-8", check=True, timeout=20).stdout


if os.name == "nt":
    import winreg

    with winreg.CreateKey(winreg.HKEY_CURRENT_USER, "Environment") as key:
        try:
            original = winreg.QueryValueEx(key, "Path")
        except FileNotFoundError:
            original = None
        try:
            with tempfile.TemporaryDirectory(prefix="prox install '") as folder:
                home = Path(folder)
                env = dict(os.environ, LOCALAPPDATA=str(home))
                # Preserve variables, value type, and long PATHs without expanding/truncating them.
                initial = r"%SystemRoot%\System32;" + ";".join(fr"C:\tools\entry{i}" for i in range(100))
                winreg.SetValueEx(key, "Path", 0, winreg.REG_EXPAND_SZ, initial)
                destination = home / "Programs" / "prox" / "prox.exe"
                destination.parent.mkdir(parents=True)
                destination.write_bytes(b"previous executable")
                assert "Installed prox" in run(EXE, env, "install")
                assert destination.read_bytes() == EXE.read_bytes()
                saved = winreg.QueryValueEx(key, "Path")
                assert saved == (initial + ";" + str(destination.parent), winreg.REG_EXPAND_SZ), saved
                run(EXE, env, "install")
                run(destination, env, "install")
                assert winreg.QueryValueEx(key, "Path") == saved
                # A fresh command interpreter resolves prox through the installed PATH.
                fresh = dict(env, PATH=str(destination.parent))
                output = run(Path(os.environ["SystemRoot"]) / "System32" / "cmd.exe", fresh,
                             "/d", "/c", "prox --version")
                assert output.startswith("prox ")
                # A missing user PATH is also supported, without copying the machine PATH.
                winreg.DeleteValue(key, "Path")
                run(EXE, env, "install")
                assert winreg.QueryValueEx(key, "Path")[0] == str(destination.parent)
                print("PASS: Windows install, upgrade, self-install, PATH preservation and command lookup")
        finally:
            if original is None:
                try:
                    winreg.DeleteValue(key, "Path")
                except FileNotFoundError:
                    pass
            else:
                winreg.SetValueEx(key, "Path", 0, original[1], original[0])
else:
    shells = [shell for name in ("bash", "zsh", "fish") if (shell := shutil.which(name))]
    assert shells
    for shell in shells:
        with tempfile.TemporaryDirectory(prefix="prox install '$ ") as folder:
            home = Path(folder)
            env = dict(os.environ, HOME=str(home), SHELL=shell,
                       ZDOTDIR=str(home), XDG_CONFIG_HOME=str(home / ".config"))
            destination = home / ("Applications/prox/bin/prox" if sys.platform == "darwin" else ".local/bin/prox")
            name = Path(shell).name
            original = "export PROX_INSTALL_PRESERVED=yes"  # Deliberately no trailing newline.
            if name == "bash":
                profile = home / ".bashrc"
                (home / ".bash_profile").write_text(original)
                (home / ".profile").write_text("# leave this alone\n")
            elif name == "zsh":
                profile = home / ".zshrc"
            else:
                profile = home / ".config/fish/conf.d/prox.fish"
                original = "set -gx PROX_INSTALL_PRESERVED yes"
            profile.parent.mkdir(parents=True, exist_ok=True)
            profile.write_text(original)
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(b"previous executable")
            assert "Installed prox" in run(EXE, env, "install")
            assert destination.read_bytes() == EXE.read_bytes()
            assert os.access(destination, os.X_OK)
            saved = profile.read_text()
            assert saved.startswith(original + "\n")
            run(EXE, env, "install")
            run(destination, env, "install")
            assert profile.read_text() == saved
            assert saved.count("# >>> prox PATH >>>") == 1
            if name == "fish":
                script = 'source "$PROX_PROFILE"; source "$PROX_PROFILE"; test "$PROX_INSTALL_PRESERVED" = yes; or exit 1; command -s prox'
            else:
                script = '. "$PROX_PROFILE"; . "$PROX_PROFILE"; test "$PROX_INSTALL_PRESERVED" = yes || exit 1; command -v prox'
            clean = dict(env, PATH="/usr/bin:/bin", PROX_PROFILE=str(profile))
            output = run(shell, clean, "-c", script)
            assert output.strip() == str(destination), output
            assert run(destination, env, "--version").startswith("prox ")
            if name == "bash":
                assert (home / ".profile").read_text() == "# leave this alone\n"
                assert "# >>> prox PATH >>>" in (home / ".bash_profile").read_text()
            print(f"PASS: {name} install, upgrade, self-install, quoting and shell config preservation")
