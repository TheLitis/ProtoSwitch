from __future__ import annotations

import argparse
import os
import re
import stat
import subprocess
import tarfile
import tempfile
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--platform", choices=["linux", "macos"], required=True)
    parser.add_argument("--arch", choices=["x64", "arm64"], required=True)
    return parser.parse_args()


def suspicious_mojibake(text: str) -> bool:
    return bool(re.search(r"[ÐÑ]|вЂ|[ЃЂѓ„…†‡€‰ЉЊЋЏђќћџ]", text))


def assert_no_mojibake(text: str, label: str) -> None:
    if suspicious_mojibake(text):
        raise SystemExit(f"Detected mojibake in {label}")


def run(command: list[str], env: dict[str, str]) -> str:
    result = subprocess.run(
        command,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        env=env,
    )
    if result.returncode != 0:
        rendered = (result.stderr or result.stdout).strip()
        raise SystemExit(
            f"{' '.join(command)} failed with exit code {result.returncode}. {rendered}"
        )
    return (result.stdout or result.stderr).strip()


def portable_archive(repo_root: Path, version: str, platform_name: str, arch: str) -> Path:
    return (
        repo_root
        / "dist"
        / version
        / f"protoswitch-portable-{platform_name}-{arch}.tar.gz"
    )


def expected_autostart_path(
    home: Path,
    platform_name: str,
    xdg_config_home: Path | None,
) -> Path:
    if platform_name == "linux":
        config_home = xdg_config_home or home / ".config"
        return config_home / "autostart" / "protoswitch.desktop"

    return home / "Library" / "LaunchAgents" / "com.thelitis.protoswitch.plist"


def extract_reported_path(output: str, label: str) -> Path:
    match = re.search(rf"(?m)^{re.escape(label)}:\s+(.+)$", output)
    if not match:
        raise SystemExit(f"{label} path not found in command output")
    return Path(match.group(1).strip())


def main() -> None:
    args = parse_args()
    repo_root = Path(args.repo_root).resolve()
    archive_path = portable_archive(repo_root, args.version, args.platform, args.arch)
    if not archive_path.exists():
        raise SystemExit(f"Portable archive not found: {archive_path}")

    with tempfile.TemporaryDirectory(prefix=f"ProtoSwitch-{args.platform}-smoke-") as tmp:
        temp_root = Path(tmp)
        with tarfile.open(archive_path, "r:gz") as archive:
            archive.extractall(temp_root)

        stage_root = temp_root / "ProtoSwitch"
        binary = stage_root / "protoswitch"
        binary.chmod(binary.stat().st_mode | stat.S_IXUSR)

        home = temp_root / "home"
        home.mkdir(parents=True, exist_ok=True)

        env = os.environ.copy()
        env["HOME"] = str(home)
        env["PYTHONUTF8"] = "1"
        xdg_config_home: Path | None = None
        xdg_data_home: Path | None = None
        if args.platform == "linux":
            xdg_config_home = temp_root / "xdg-config"
            xdg_data_home = temp_root / "xdg-data"
            xdg_config_home.mkdir(parents=True, exist_ok=True)
            xdg_data_home.mkdir(parents=True, exist_ok=True)
            env["XDG_CONFIG_HOME"] = str(xdg_config_home)
            env["XDG_DATA_HOME"] = str(xdg_data_home)
            env["LC_ALL"] = env.get("LC_ALL", "C.UTF-8")
            env["LANG"] = env.get("LANG", "C.UTF-8")
        else:
            env["LC_ALL"] = env.get("LC_ALL", "en_US.UTF-8")
            env["LANG"] = env.get("LANG", "en_US.UTF-8")

        for doc_name in ("README.md", "CHANGELOG.md", "QUICKSTART.txt"):
            assert_no_mojibake(
                (stage_root / doc_name).read_text(encoding="utf-8-sig"), doc_name
            )

        run([str(binary), "--version"], env)
        init_output = run([str(binary), "init", "--non-interactive", "--no-autostart"], env)
        status_output = run([str(binary), "status", "--plain"], env)
        doctor_output = run([str(binary), "doctor"], env)
        assert_no_mojibake(status_output, "status")
        assert_no_mojibake(doctor_output, "doctor")
        if "Статус proxy" not in status_output:
            raise SystemExit("Portable smoke expected plain status output.")

        config_path = extract_reported_path(init_output, "config.toml")
        state_path = extract_reported_path(init_output, "state.json")
        autostart_path = expected_autostart_path(home, args.platform, xdg_config_home)
        if not config_path.exists():
            raise SystemExit(f"Config file not found after init: {config_path}")
        if not state_path.exists():
            raise SystemExit(f"State file not found after init: {state_path}")

        run([str(binary), "autostart", "install"], env)
        if not autostart_path.exists():
            raise SystemExit(f"Autostart file not created: {autostart_path}")

        run([str(binary), "autostart", "remove"], env)
        if autostart_path.exists():
            raise SystemExit(f"Autostart file still exists: {autostart_path}")

        run([str(binary), "shutdown"], env)

    print(f"{args.platform}-{args.arch} portable smoke completed")


if __name__ == "__main__":
    main()
