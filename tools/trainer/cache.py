"""Content-addressed dictionary builds; never stores articles or typing results."""

import fcntl
import hashlib
import os
import shutil
import subprocess
import tempfile
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class Snapshot:
    path: Path
    reused: bool
    key: str


def sources(root: Path) -> list[Path]:
    files = [
        p
        for p in root.iterdir()
        if p.is_file() and p.suffix in (".yaml", ".lua", ".txt")
    ]
    for name in ("lua", "opencc"):
        if (root / name).is_dir():
            files.extend(p for p in (root / name).rglob("*") if p.is_file())
    return sorted(files)


def fingerprint(root: Path, shared: Path) -> str:
    digest = hashlib.sha256(b"xkjd-type-cache-v1")
    for label, directory in (("user", root), ("shared", shared)):
        if not directory.exists():
            continue
        for path in sources(directory):
            digest.update(f"{label}/{path.relative_to(directory)}\0".encode())
            with path.open("rb") as stream:
                for block in iter(lambda: stream.read(1024 * 1024), b""):
                    digest.update(block)
    for binary in ("/usr/bin/rime_deployer", "/usr/lib/librime.so"):
        path = Path(binary)
        if path.exists():
            stat = path.stat()
            digest.update(
                f"{path.resolve()}:{stat.st_size}:{stat.st_mtime_ns}".encode()
            )
    return digest.hexdigest()


def deploy(root: Path) -> None:
    with (root.parent / "compile.log").open("w") as log:
        result = subprocess.run(
            [
                "rime_deployer",
                "--build",
                str(root),
                "/usr/share/rime-data",
                str(root / "build"),
            ],
            stdout=log,
            stderr=log,
            timeout=180,
            check=False,
        )
    if result.returncode or not (root / "build/xkjd6.schema.yaml").exists():
        raise RuntimeError("隔离词库编译失败；请检查 Rime 源文件")


def prepare(
    root: Path,
    cache: Path,
    *,
    shared: Path = Path("/usr/share/rime-data"),
    build: Callable[[Path], None] = deploy,
) -> Snapshot:
    root = root.resolve()
    cache.mkdir(parents=True, exist_ok=True, mode=0o700)
    with (cache / "compile.lock").open("a+") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        key = fingerprint(root, shared)
        destination = cache / key
        if (destination / "ready").exists():
            return Snapshot(destination / "rime", True, key)
        with tempfile.TemporaryDirectory(prefix=".build-", dir=cache) as work:
            staging = Path(work)
            data = staging / "rime"
            data.mkdir()
            for source in sources(root):
                target = data / source.relative_to(root)
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(source, target)
            # Source-only builds avoid reusing stale deployment products.
            build(data)
            if fingerprint(root, shared) != key:
                raise RuntimeError("词库在编译过程中发生变化，请重试")
            (staging / "ready").touch()
            os.rename(staging, destination)
        return Snapshot(destination / "rime", False, key)
