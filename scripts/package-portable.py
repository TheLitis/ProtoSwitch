from __future__ import annotations

import argparse
import shutil
import tarfile
import zipfile
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--platform", required=True)
    parser.add_argument("--arch", required=True)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--format", choices=["zip", "tar.gz"], required=True)
    return parser.parse_args()


def portable_name(platform_name: str, arch: str, archive_format: str) -> str:
    suffix = "zip" if archive_format == "zip" else "tar.gz"
    return f"protoswitch-portable-{platform_name}-{arch}.{suffix}"


def binary_name(platform_name: str) -> str:
    return "protoswitch.exe" if platform_name == "win" else "protoswitch"


def build_stage(repo_root: Path, version: str, platform_name: str, arch: str, binary: Path) -> Path:
    dist_root = repo_root / "dist" / version
    stage_root = dist_root / "portable-stage" / f"{platform_name}-{arch}" / "ProtoSwitch"
    if stage_root.parent.exists():
        shutil.rmtree(stage_root.parent)
    stage_root.mkdir(parents=True, exist_ok=True)

    shutil.copy2(binary, stage_root / binary_name(platform_name))
    shutil.copy2(repo_root / "README.md", stage_root / "README.md")
    shutil.copy2(repo_root / "CHANGELOG.md", stage_root / "CHANGELOG.md")
    shutil.copy2(repo_root / "packaging" / "portable" / "QUICKSTART.txt", stage_root / "QUICKSTART.txt")
    return stage_root


def create_zip(stage_root: Path, destination: Path) -> None:
    with zipfile.ZipFile(destination, "w", zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(stage_root.parent.rglob("*")):
            if path.is_file():
                archive.write(path, path.relative_to(stage_root.parent))


def create_tar_gz(stage_root: Path, destination: Path) -> None:
    with tarfile.open(destination, "w:gz") as archive:
        archive.add(stage_root, arcname=stage_root.name)


def main() -> None:
    args = parse_args()
    repo_root = Path(args.repo_root).resolve()
    binary = Path(args.binary).resolve()
    if not binary.exists():
        raise SystemExit(f"Binary not found: {binary}")

    dist_root = repo_root / "dist" / args.version
    dist_root.mkdir(parents=True, exist_ok=True)
    archive_path = dist_root / portable_name(args.platform, args.arch, args.format)
    stage_root = build_stage(repo_root, args.version, args.platform, args.arch, binary)

    if archive_path.exists():
        archive_path.unlink()

    if args.format == "zip":
        create_zip(stage_root, archive_path)
    else:
        create_tar_gz(stage_root, archive_path)

    shutil.rmtree(stage_root.parent)
    print(archive_path)


if __name__ == "__main__":
    main()
