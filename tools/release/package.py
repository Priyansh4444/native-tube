#!/usr/bin/env python3
"""Build, package and verify NativeTube release assets."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[2]
FRONTENDS = ("iced", "gpui", "rune")
TARGETS = {
    "linux-x86_64": ("Linux", "x86_64", "x86_64-unknown-linux-gnu"),
    "macos-arm64": ("Darwin", "arm64", "aarch64-apple-darwin"),
}
LICENSE_NAME = re.compile(r"^(licen[sc]es?|copying|copyright|notice|ofl|unlicense)($|[._-])", re.I)


def run(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def sha256(file):
    digest = hashlib.sha256()
    for chunk in iter(lambda: file.read(1024 * 1024), b""):
        digest.update(chunk)
    return digest.hexdigest()


def checksum(archive):
    with archive.open("rb") as file:
        return f"{sha256(file)}  {archive.name}\n"


def notices(directory):
    return (file for file in directory.iterdir()
            if file.is_file() and LICENSE_NAME.match(file.name))


def copy_notice(file, directory, name):
    directory.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(file, directory / name)


def write_attribution(stage, manifest, triple, candidate):
    command = ["cargo", "metadata", "--locked", "--format-version", "1",
               "--filter-platform", triple, "--manifest-path", str(manifest)]
    if candidate:
        command.append("--offline")
    metadata = json.loads(run(*command))
    resolve = metadata["resolve"]
    nodes = {node["id"]: node["deps"] for node in resolve["nodes"]}
    pending = [resolve["root"]]
    reachable = set()
    while pending:
        package_id = pending.pop()
        if package_id in reachable:
            continue
        reachable.add(package_id)
        pending.extend(dep["pkg"] for dep in nodes[package_id]
                       if any(kind["kind"] != "dev" for kind in dep["dep_kinds"]))

    licenses = stage / "licenses"
    for file in sorted((*notices(ROOT), *notices(ROOT / "assets" / "fonts"),
                        *notices(ROOT / "assets" / "icons"))):
        copy_notice(file, licenses / "project", str(file.relative_to(ROOT)).replace("/", "_"))

    if not candidate:
        rust_docs = Path(run("rustc", "--print", "sysroot")) / "share" / "doc" / "rust"
        if (rust_docs / "COPYRIGHT-library.html").is_file():
            copy_notice(rust_docs / "COPYRIGHT-library.html", licenses / "rust",
                        "COPYRIGHT-library.html")
            if (rust_docs / "licenses").is_dir():
                shutil.copytree(rust_docs / "licenses", licenses / "rust" / "licenses")

    lines = [
        "# Dependency attribution\n",
        "Cargo resolved build and runtime dependency graph for " + triple + ".",
        "The graph includes build tools and packages that may contribute no code to the executable.",
        "Cargo license expressions and source locations are listed below. Available notice files",
        "are under licenses/. Cargo.lock records pinned versions and source revisions.",
        "The executable also uses the Rust standard library. See",
        "https://github.com/rust-lang/rust#license. Host graphics libraries and libmpv are installed separately.",
    ]
    if candidate:
        lines.append("Local candidate: this inventory describes the packaging checkout only.")

    packages = sorted(metadata["packages"], key=lambda pkg: (pkg["name"], pkg["version"], pkg["id"]))
    for index, pkg in enumerate(packages, 1):
        if pkg["id"] not in reachable:
            continue
        directory = Path(pkg["manifest_path"]).parent
        source = pkg["source"] or str(directory.relative_to(ROOT))
        dest = licenses / "cargo" / f"{pkg['name']}-{pkg['version']}-{index}"
        # Crates can vendor native libraries with their own nested notices (for example AWS-LC).
        for folder, subdirs, names in os.walk(directory):
            subdirs[:] = sorted(set(subdirs) - {".git", "target"})
            for name in sorted(names):
                file = Path(folder) / name
                if not LICENSE_NAME.match(name) or not file.is_file():
                    continue
                relative = file.relative_to(directory)
                copy_notice(file, dest / relative.parent, relative.name)

        if pkg["license_file"]:
            declared = directory / pkg["license_file"]
            if declared.is_file():
                copy_notice(declared, dest, f"declared-{declared.name}")

        if source.startswith("git+"):
            # Git workspace licensing can live beside the top-level, rather than crate, manifest.
            git_root = next(parent for parent in (directory, *directory.parents)
                            if (parent / ".git").exists())
            if git_root != directory:
                for file in sorted(notices(git_root)):
                    copy_notice(file, dest, f"workspace-{file.name}")

        notice_path = f"; {dest.relative_to(stage)}" if dest.exists() else ""
        license_expression = pkg["license"] or "see supplied license file or upstream source"
        lines.append(f"- {pkg['name']} {pkg['version']}: {license_expression}; {source}{notice_path}")

    (stage / "DEPENDENCIES.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


def inspect_binary(binary, target):
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValueError(f"Missing executable or execute permission: {binary}")
    if target == "linux-x86_64":
        header = run("readelf", "--file-header", str(binary))
        if not re.search(r"Machine:\s+Advanced Micro Devices X86-64", header):
            raise ValueError(f"Expected x86_64 ELF executable: {binary}")
        dynamic = run("readelf", "--dynamic", str(binary))
        libs = re.findall(r"\(NEEDED\).*\[(.*?)\]", dynamic)
        versions = re.findall(r"\bGLIBC_([0-9.]+)",
                              run("readelf", "--version-info", str(binary)))
        if not versions:
            raise ValueError(f"Could not determine glibc requirements: {binary}")
        floor = max(versions, key=lambda value: tuple(map(int, value.split("."))))
        return {"linked_libraries": libs, "maximum_glibc_symbol_version": floor}

    if run("lipo", "-archs", str(binary)) != "arm64":
        raise ValueError(f"Expected a native arm64 Mach-O executable: {binary}")
    libs = [line.strip() for line in run("otool", "-L", str(binary)).splitlines()[1:]]
    minimum = re.search(r"\bminos\s+(\S+)", run("xcrun", "vtool", "-show-build", str(binary)))
    if minimum is None:
        raise ValueError(f"Could not determine Mach-O deployment target: {binary}")
    return {"linked_libraries": libs, "mach_o_minimum_os": minimum.group(1)}


def package(frontend, target, output, candidate):
    os_name, machine, triple = TARGETS[target]
    if (platform.system(), platform.machine()) != (os_name, machine):
        raise ValueError(f"Packaging requires a native {target} host")
    revision = run("git", "rev-parse", "HEAD")
    dirty = bool(run("git", "status", "--porcelain"))
    if not candidate:
        if dirty:
            raise ValueError("A fresh package requires a clean checkout")
        if os.environ.get("GITHUB_ACTIONS") == "true":
            event_commit = run("git", "rev-parse", os.environ["GITHUB_SHA"] + "^{commit}")
            if event_commit != revision:
                raise ValueError(f"Checked out {revision}, expected {event_commit}")

    binary_name = f"nt-{frontend}"
    name = f"native-tube-{frontend}-{target}"
    if candidate:
        name += "-local-candidate"
    manifest = ROOT / "apps" / frontend / "Cargo.toml"
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=output) as temporary:
        work = Path(temporary)
        command = ["cargo", "build", "--locked", "--release", "--target", triple,
                   "--manifest-path", str(manifest.relative_to(ROOT)), "--bin", binary_name]
        if candidate:
            binary = candidate.resolve()
            if candidate.name != binary_name:
                raise ValueError(f"Expected executable named {binary_name}: {candidate}")
        else:
            build_dir = work / "build"
            subprocess.run([*command, "--target-dir", str(build_dir)], cwd=ROOT, check=True)
            binary = build_dir / triple / "release" / binary_name
        runtime = inspect_binary(binary, target)

        stage = work / name
        stage.mkdir()
        shutil.copyfile(binary, stage / binary_name)
        (stage / binary_name).chmod(0o755)
        shutil.copyfile(manifest.with_name("Cargo.lock"), stage / "Cargo.lock")
        write_attribution(stage, manifest, triple, candidate)

        instructions = (ROOT / "tools" / "release" / f"{os_name.lower()}.txt").read_text()
        heading = f"NativeTube {frontend} | {target}\n\n"
        if candidate:
            heading += ("LOCAL PACKAGING CANDIDATE\nThe supplied executable may predate the checkout. "
                        "Its original build revision and toolchain are unknown.\n\n")
        instructions = instructions.replace("@BINARY@", binary_name)
        (stage / "RUN.txt").write_text(heading + instructions, encoding="utf-8")

        with (stage / binary_name).open("rb") as file:
            binary_digest = sha256(file)
        provenance = {
            "frontend": frontend,
            "target": target,
            "rust_target": triple,
            "binary_sha256": binary_digest,
            "package_kind": "local-candidate" if candidate else "fresh-build",
            "source_revision": None if candidate else revision,
            "packaging_checkout_revision": revision,
            "packaging_checkout_dirty": dirty,
            "source_ref": os.environ.get("GITHUB_REF"),
            "github_run_id": os.environ.get("GITHUB_RUN_ID"),
            "github_run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
            "github_repository": os.environ.get("GITHUB_REPOSITORY"),
            "packaged_at_utc": datetime.now(timezone.utc).isoformat(),
            "packaging_host": platform.platform(),
            "runner_image": os.environ.get("ImageOS"),
            "runner_image_version": os.environ.get("ImageVersion"),
            "rustc": None if candidate else run("rustc", "-vV"),
            "cargo": None if candidate else run("cargo", "--version"),
            "build_command": None if candidate else command + ["--target-dir", "<temporary>"],
            **runtime,
        }
        (stage / "BUILD.json").write_text(json.dumps(provenance, indent=2) + "\n", encoding="utf-8")

        archive = work / f"{name}.tar.gz"
        with tarfile.open(archive, "w:gz", compresslevel=3) as tar:
            tar.add(stage, arcname=name)
        digest = work / f"{archive.name}.sha256"
        digest.write_text(checksum(archive), encoding="ascii")
        archive.replace(output / archive.name)
        digest.replace(output / digest.name)
        print(output / archive.name)


def verify(directory, commit, ref, run_id):
    expected = {f"native-tube-{frontend}-{target}": (frontend, target)
                for frontend in FRONTENDS for target in TARGETS}
    if {path.name for path in directory.iterdir()} != set(expected):
        raise ValueError(f"Expected six artifact folders: {', '.join(sorted(expected))}")

    for name, (frontend, target) in sorted(expected.items()):
        folder = directory / name
        archive = folder / f"{name}.tar.gz"
        digest = folder / f"{archive.name}.sha256"
        if set(folder.iterdir()) != {archive, digest}:
            raise ValueError(f"Expected archive and checksum only in {folder}")
        if digest.read_text(encoding="ascii") != checksum(archive):
            raise ValueError(f"SHA256 mismatch: {archive}")

        with tarfile.open(archive, "r:gz") as tar:
            binary = tar.getmember(f"{name}/nt-{frontend}")
            if not binary.isfile() or not binary.mode & 0o111:
                raise ValueError(f"Missing executable permission: {archive}")
            for required in ("RUN.txt", "Cargo.lock", "DEPENDENCIES.md", "licenses"):
                tar.getmember(f"{name}/{required}")
            with tar.extractfile(f"{name}/BUILD.json") as file:
                provenance = json.load(file)
            with tar.extractfile(binary) as file:
                binary_digest = sha256(file)
        expected_provenance = {
            "frontend": frontend, "target": target, "binary_sha256": binary_digest,
            "package_kind": "fresh-build", "source_revision": commit, "source_ref": ref,
            "github_run_id": run_id,
        }
        if any(provenance.get(key) != value for key, value in expected_provenance.items()):
            raise ValueError(f"Build provenance mismatch: {archive}")
        print(f"Verified {archive.name}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build = commands.add_parser("package", help="Build and package one frontend")
    build.add_argument("frontend", choices=FRONTENDS)
    build.add_argument("target", choices=TARGETS)
    build.add_argument("output", type=Path)
    build.add_argument("--candidate", type=Path, help="Package an existing executable for local review")
    check = commands.add_parser("verify", help="Verify all six downloaded CI asset pairs")
    check.add_argument("directory", type=Path)
    check.add_argument("commit")
    check.add_argument("ref")
    check.add_argument("run_id")
    args = parser.parse_args()
    try:
        if args.command == "package":
            package(args.frontend, args.target, args.output, args.candidate)
        else:
            verify(args.directory, args.commit, args.ref, args.run_id)
    except (ValueError, OSError, KeyError, tarfile.TarError, subprocess.CalledProcessError) as exc:
        parser.exit(1, f"{exc}\n")


if __name__ == "__main__":
    main()
