"""Launch a session-local Fcitx/Rime backend and a font-preserving Konsole CLI trainer."""

from __future__ import annotations

import argparse
import json
import os
import random
import resource
import select
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unicodedata
import uuid
from contextlib import ExitStack
from pathlib import Path

import yaml
from cache import prepare
from solver import solve_references

REPOSITORY = Path(__file__).resolve().parents[2]
SCRIPT_DIR = Path(__file__).resolve().parent
DEFAULT_MEMORY_MIB = 2048
ARTICLE_SUFFIXES = frozenset({".txt", ".md", ".markdown"})


def bounded_run(memory_mib: int) -> int:
    """A separate user cgroup limits the complete trainer process tree."""
    unit = "xkjd-type-" + uuid.uuid4().hex + ".scope"
    command = [
        "systemd-run",
        "--user",
        "--scope",
        "--quiet",
        "--collect",
        "--unit",
        unit,
        "-p",
        f"MemoryMax={memory_mib}M",
        "-p",
        "MemorySwapMax=0",
        "-p",
        "TasksMax=64",
        sys.executable,
        str(SCRIPT_DIR / "launcher.py"),
        "--bounded",
        *sys.argv[1:],
    ]
    process = subprocess.Popen(command)
    try:
        return process.wait()
    finally:
        # Stopping the scope also reaps native children after an abnormal exit.
        subprocess.run(
            ["systemctl", "--user", "stop", unit],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=10,
            check=False,
        )


def terminate(_signum: int, _frame: object) -> None:
    raise SystemExit(143)


def stop(process: subprocess.Popen[str]) -> None:
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def select_article(path: Path) -> Path:
    if not path.is_dir():
        return path
    candidates = [
        entry
        for entry in path.rglob("*")
        if entry.suffix.lower() in ARTICLE_SUFFIXES and entry.is_file()
    ]
    if not candidates:
        raise ValueError(f"目录中没有可跟打的 .txt、.md 或 .markdown 文件：{path}")
    selected = random.choice(candidates)
    print(f"随机加载：{selected}", flush=True)
    return selected


def load_text(args: argparse.Namespace) -> str:
    text = (
        args.text
        if args.text is not None
        else args.file.read_text(encoding="utf-8-sig")
    )
    text = text.replace("\r\n", "\n").replace("\r", "\n").strip("\n")
    if args.file and args.paragraph is not None:
        import re

        paragraphs = [part for part in re.split(r"\n[ \t]*\n+", text) if part.strip()]
        if not 1 <= args.paragraph <= len(paragraphs):
            raise ValueError(f"段落编号超出范围：文件共 {len(paragraphs)} 段")
        text = paragraphs[args.paragraph - 1]
        print(f"加载第 {args.paragraph}/{len(paragraphs)} 段", flush=True)
    if not text:
        raise ValueError("跟打段落不能为空")
    if any(
        unicodedata.category(char) == "Cc" and char not in ("\n", "\t") for char in text
    ):
        raise ValueError("段落不能包含终端控制字符")
    return text


def build_engine() -> Path:
    source = REPOSITORY / "tools/rime-sim/engine.cpp"
    executable = REPOSITORY / "target/native/rime-engine"
    if (
        not executable.exists()
        or executable.stat().st_mtime_ns < source.stat().st_mtime_ns
    ):
        executable.parent.mkdir(parents=True, exist_ok=True)
        flags = shlex.split(
            subprocess.check_output(
                ["pkg-config", "--cflags", "--libs", "rime"], text=True
            )
        )
        subprocess.run(
            [
                "g++",
                "-std=c++17",
                "-O2",
                str(source),
                "-o",
                str(executable),
                *flags,
                "-ldl",
            ],
            check=True,
            timeout=120,
        )
    return executable


def configure_practice_rime(root: Path) -> None:
    """Restrict the disposable deployment to xkjd6, initially in Chinese mode."""
    default_path = root / "build/default.yaml"
    default: object = yaml.safe_load(default_path.read_text(encoding="utf-8"))
    if not isinstance(default, dict):
        raise TypeError(f"无效的 Rime 配置：{default_path}")
    default["schema_list"] = [{"schema": "xkjd6"}]
    schema_path = root / "build/xkjd6.schema.yaml"
    schema: object = yaml.safe_load(schema_path.read_text(encoding="utf-8"))
    switches = schema.get("switches") if isinstance(schema, dict) else None
    if not isinstance(switches, list):
        raise TypeError(f"缺少或无效的 Rime switches：{schema_path}")
    ascii_switch = next(
        (
            switch
            for switch in switches
            if isinstance(switch, dict) and switch.get("name") == "ascii_mode"
        ),
        None,
    )
    if ascii_switch is None:
        raise ValueError(f"缺少 Rime ascii_mode：{schema_path}")
    ascii_switch["reset"] = 0
    default_path.write_text(
        yaml.safe_dump(default, allow_unicode=True), encoding="utf-8"
    )
    schema_path.write_text(yaml.safe_dump(schema, allow_unicode=True), encoding="utf-8")


def snapshot(root: Path, session: Path) -> Path:
    cache = (
        Path(os.environ.get("XDG_CACHE_HOME", str(Path.home() / ".cache")))
        / "xkjd-type/dictionaries"
    )
    cached = prepare(root, cache)
    print("复用词库编译缓存" if cached.reused else "词库编译完成", flush=True)
    destination = session / "data/fcitx5/rime"
    shutil.copytree(cached.path, destination)
    configure_practice_rime(destination)
    config = session / "config/fcitx5"
    config.mkdir(parents=True)
    (config / "profile").write_text(
        "[Groups/0]\nName=Default\nDefault Layout=us\nDefaultIM=rime\n\n[Groups/0/Items/0]\nName=rime\nLayout=\n\n[GroupOrder]\n0=Default\n"
    )
    actual_config = (
        Path(os.environ.get("XDG_CONFIG_HOME", str(Path.home() / ".config")))
        / "fcitx5/conf"
    )
    (config / "conf").mkdir()
    for name in ("rime.conf", "classicui.conf"):
        if (actual_config / name).is_file():
            shutil.copy2(actual_config / name, config / "conf" / name)
    return destination


def launch(session: Path) -> None:
    # No desktop service activation: private Qt clients must not start extra
    # portal/keyring/accessibility daemons on this throwaway bus.
    bus_config = session / "bus.conf"
    bus_config.write_text("""<busconfig>
<type>session</type><listen>unix:tmpdir=/tmp</listen><auth>EXTERNAL</auth>
<policy context="default"><allow send_destination="*" eavesdrop="true"/>
<allow eavesdrop="true"/><allow own="*"/></policy>
</busconfig>""")
    with ExitStack() as cleanup:
        bus_error = cleanup.enter_context((session / "bus.stderr.log").open("w"))
        bus = subprocess.Popen(
            [
                "dbus-daemon",
                f"--config-file={bus_config}",
                "--nofork",
                "--print-address=1",
            ],
            stdout=subprocess.PIPE,
            stderr=bus_error,
            text=True,
        )
        cleanup.callback(stop, bus)
        assert bus.stdout is not None
        if not select.select([bus.stdout], [], [], 5)[0]:
            raise RuntimeError("私有 D-Bus 启动超时")
        address = bus.stdout.readline().strip()
        if not address:
            raise RuntimeError("未取得私有 D-Bus 地址")
        server_env = dict(
            os.environ,
            DBUS_SESSION_BUS_ADDRESS=address,
            XDG_CONFIG_HOME=str(session / "config"),
            XDG_DATA_HOME=str(session / "data"),
            XDG_CACHE_HOME=str(session / "cache"),
        )
        client_env = dict(
            os.environ,
            DBUS_SESSION_BUS_ADDRESS=address,
            QT_IM_MODULE="fcitx",
            QT_IM_MODULES="fcitx",
        )
        fcitx_log = cleanup.enter_context((session / "fcitx.log").open("w"))
        server = subprocess.Popen(
            [
                "fcitx5",
                "-D",
                "-k",
                "--disable=all",
                "--enable=dbus,dbusfrontend,keyboard,rime,classicui,xcb",
                "--verbose=key_trace=5",
            ],
            env=server_env,
            stdout=fcitx_log,
            stderr=fcitx_log,
            text=True,
        )
        cleanup.callback(stop, server)

        def remote(path: str, member: str, *values: str, check: bool = True):
            result = subprocess.run(
                ["qdbus6", "org.fcitx.Fcitx5", path, member, *values],
                env=server_env,
                capture_output=True,
                text=True,
                timeout=5,
                check=False,
            )
            if check and result.returncode:
                raise RuntimeError(f"{member}: {result.stderr.strip()}")
            return result

        deadline = time.monotonic() + 12
        while remote(
            "/controller", "org.fcitx.Fcitx.Controller1.State", check=False
        ).returncode:
            if server.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError(f"隔离 fcitx 启动失败，见 {session / 'fcitx.log'}")
            time.sleep(0.1)
        capture_error = cleanup.enter_context(
            (session / "capture.stderr.log").open("w")
        )
        capture = subprocess.Popen(
            [
                sys.executable,
                str(SCRIPT_DIR / "capture_bus.py"),
                "--address",
                address,
                "--output",
                str(session / "events.jsonl"),
            ],
            stdout=capture_error,
            stderr=capture_error,
            text=True,
        )
        cleanup.callback(stop, capture)
        deadline = time.monotonic() + 5
        while (
            not (session / "events.jsonl").exists()
            or not (session / "events.jsonl").stat().st_size
        ):
            if capture.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError(
                    f"按键采集器启动失败，见 {session / 'capture.stderr.log'}"
                )
            time.sleep(0.05)
        terminal_error = cleanup.enter_context(
            (session / "terminal.stderr.log").open("w")
        )
        terminal = subprocess.Popen(
            [
                "konsole",
                "--separate",
                "-p",
                "LocalTabTitleFormat=键道跟打",
                "-e",
                sys.executable,
                str(SCRIPT_DIR / "launcher.py"),
                "--worker",
                str(session),
            ],
            env=client_env,
            stdout=terminal_error,
            stderr=terminal_error,
            text=True,
        )
        cleanup.callback(stop, terminal)
        deadline = time.monotonic() + 15
        while not (session / "worker-ready").exists():
            if terminal.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError(
                    f"跟打终端未就绪，见 {session / 'terminal.stderr.log'}"
                )
            time.sleep(0.05)
        print("请切到新打开的跟打窗口，等待输入法初始化。", flush=True)
        deadline = time.monotonic() + 90
        while True:
            debug = remote(
                "/controller", "org.fcitx.Fcitx.Controller1.DebugInfo"
            ).stdout
            if any(
                "konsole" in line and "frontend:dbus" in line and "focus:1" in line
                for line in debug.splitlines()
            ):
                (session / "input-context.txt").write_text(debug)
                break
            if terminal.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError(
                    "未发现跟打终端的私有 fcitx 输入上下文；请确认终端窗口获得焦点"
                )
            time.sleep(0.15)
        remote("/controller", "org.fcitx.Fcitx.Controller1.SetCurrentIM", "rime")
        remote("/rime", "org.fcitx.Fcitx.Rime1.SetSchema", "xkjd6")
        remote("/rime", "org.fcitx.Fcitx.Rime1.SetAsciiMode", "false")
        (session / "ime-ready").write_text("ready")
        print(
            "跟打窗口已就绪。F2 / Ctrl+D 退出；完成轮次和参考日志保留在 /tmp。",
            flush=True,
        )
        while terminal.poll() is None:
            if capture.poll() is not None:
                raise RuntimeError(f"采集器中断，见 {session / 'capture.stderr.log'}")
            if server.poll() is not None:
                raise RuntimeError(f"隔离输入法中断，见 {session / 'fcitx.log'}")
            time.sleep(0.2)
    if (session / "worker-failure").exists():
        raise RuntimeError((session / "worker-failure").read_text())


def main() -> None:
    parser = argparse.ArgumentParser(
        description="键道 CLI 跟打：后台算码、完成后对比；完成轮次日志留在 /tmp"
    )
    source = parser.add_mutually_exclusive_group()
    source.add_argument(
        "--file",
        type=Path,
        help="UTF-8 文件或目录；目录中递归随机选一篇 txt/md/markdown，默认加载全文",
    )
    source.add_argument("--text", help="直接指定跟打内容")
    source.add_argument(
        "--report-log",
        type=Path,
        help="读取完成轮次日志，输出原文、实际与两组参考对照，不开启输入法",
    )
    parser.add_argument(
        "--paragraph", type=int, help="可选：仅加载文件中第 N 段，空行分隔，从 1 开始"
    )
    parser.add_argument(
        "--root",
        type=Path,
        default=Path.home() / ".local/share/fcitx5/rime",
        help="只读 Rime 词库目录",
    )
    parser.add_argument(
        "--plan-only", action="store_true", help="只在标准输出显示两种参考，不打开窗口"
    )
    parser.add_argument(
        "--max-states", type=int, default=10000, help="每种参考、每句段最多扩展状态"
    )
    parser.add_argument(
        "--search-seconds",
        type=float,
        default=45,
        help="每种参考、每句段搜索秒数，默认 45",
    )
    parser.add_argument(
        "--memory-mib",
        type=int,
        default=DEFAULT_MEMORY_MIB,
        help="进程树内存上限 MiB，默认 2048，不用交换空间",
    )
    parser.add_argument("--worker", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--reference-worker", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--bounded", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--test-runtime-parent", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    signal.signal(signal.SIGTERM, terminate)
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    if args.worker:
        from tui import run

        run(args.worker.resolve())
        return
    if args.reference_worker:
        from planning import worker

        worker(args.reference_worker)
        return
    if args.file is None and args.text is None and args.report_log is None:
        parser.error("请指定 --file、--text 或 --report-log")
    if args.paragraph is not None and (args.file is None or args.paragraph < 1):
        parser.error("--paragraph 必须与 --file 一起使用，且至少为 1")
    if args.max_states < 1 or not 0 < args.search_seconds <= 300:
        parser.error("搜索限制必须为正，秒数不超过 300")
    if args.memory_mib < 512:
        parser.error("--memory-mib 至少为 512")
    if not args.bounded:
        raise SystemExit(bounded_run(args.memory_mib))
    if args.report_log is not None:
        from analysis import compare
        from recording import load_round
        from rich.console import Console
        from view import results

        record = load_round(args.report_log.resolve())
        comparison = (
            compare(record.buffer, record.native, record.plan.references)
            if record.plan.references is not None
            else None
        )
        console = Console(no_color=False)
        console.print("完成轮次日志：", str(args.report_log.resolve()), markup=False)
        console.print(
            "来源：本人输入"
            if record.origin == "user"
            else "来源：程序回放（非本人实测）"
        )
        console.print(
            results(
                record.native,
                record.buffer,
                record.plan,
                comparison,
                width=console.width,
            )
        )
        return
    if args.file is not None:
        args.file = select_article(args.file).resolve()
    target = load_text(args)
    runtime = args.test_runtime_parent or Path(
        os.environ.get("XDG_RUNTIME_DIR", tempfile.gettempdir())
    )
    runtime.mkdir(parents=True, exist_ok=True, mode=0o700)
    log_directory = Path(tempfile.mkdtemp(prefix="xkjd-type-log-", dir="/tmp"))
    print(f"诊断日志：{log_directory}", flush=True)
    (log_directory / "session.json").write_text(
        json.dumps(
            {
                "source_path": str(args.file) if args.file is not None else None,
                "max_states": args.max_states,
                "search_seconds": args.search_seconds,
            },
            ensure_ascii=False,
        ),
        encoding="utf-8",
    )
    with tempfile.TemporaryDirectory(prefix="xkjd-type-", dir=runtime) as directory:
        session = Path(directory)
        print("准备输入法…", flush=True)
        root = snapshot(args.root, session)
        shutil.copytree(root, session / "oracle")
        executable = build_engine()
        if args.plan_only:
            reference = solve_references(
                target,
                session / "oracle",
                executable,
                log_directory / "oracle.log",
                args.max_states,
                args.search_seconds,
            )
            print(json.dumps(reference, ensure_ascii=False, indent=2))
            return
        (session / "session.json").write_text(
            json.dumps(
                {
                    "target": target,
                    "source_path": str(args.file) if args.file is not None else None,
                    "log_directory": str(log_directory),
                    "executable": str(executable),
                    "max_states": args.max_states,
                    "search_seconds": args.search_seconds,
                    "test_events": args.test_runtime_parent is not None,
                },
                ensure_ascii=False,
            )
        )
        launch(session)


if __name__ == "__main__":
    main()
