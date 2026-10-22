"""Cancelable background references; IPC lives only in the temporary runtime."""

import json
import os
import signal
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO, Literal

from solver import References, solve_references


@dataclass
class PlanState:
    status: Literal["pending", "ready", "error"] = "pending"
    references: References | None = None
    error: str | None = None


class Planner:
    def __init__(self, session: Path):
        self.session = session
        spec = json.loads((session / "session.json").read_text())
        self.log_directory = Path(spec["log_directory"])
        self.state = PlanState()
        self.process: subprocess.Popen[bytes] | None = None
        self.log: BinaryIO | None = None

    def start(self, target: str) -> None:
        self.close()
        self.state = PlanState()
        for name in ("reference-ready.json", "reference-error"):
            (self.session / name).unlink(missing_ok=True)
        (self.session / "reference-target.json").write_text(
            json.dumps(target, ensure_ascii=False)
        )
        self.log = (self.log_directory / "reference.log").open("ab")
        self.process = subprocess.Popen(
            [
                sys.executable,
                str(Path(__file__).with_name("launcher.py")),
                "--reference-worker",
                str(self.session),
            ],
            stdout=self.log,
            stderr=self.log,
            start_new_session=True,
        )

    def poll(self) -> PlanState:
        if self.process is None or self.state.status != "pending":
            return self.state
        ready = self.session / "reference-ready.json"
        if ready.exists():
            self.state = PlanState("ready", json.loads(ready.read_text()))
        elif self.process.poll() is not None:
            error = self.session / "reference-error"
            self.state = PlanState(
                "error",
                error=error.read_text()
                if error.exists()
                else f"参考进程退出：{self.process.returncode}",
            )
        return self.state

    def close(self) -> None:
        if self.process is not None:
            if self.process.poll() is None:
                self.signal(signal.SIGTERM)
                try:
                    self.process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    self.signal(signal.SIGKILL)
                    self.process.wait(timeout=2)
            self.process = None
        if self.log is not None:
            self.log.close()
            self.log = None

    def signal(self, value: signal.Signals) -> None:
        if self.process is not None:
            try:
                os.killpg(self.process.pid, value)
            except ProcessLookupError:
                pass  # The owned group exited between poll() and killpg().


def worker(session: Path) -> None:
    spec = json.loads((session / "session.json").read_text())
    log_directory = Path(spec["log_directory"])
    target = json.loads((session / "reference-target.json").read_text())
    try:
        references = solve_references(
            target,
            session / "oracle",
            Path(spec["executable"]),
            log_directory / "oracle.log",
            spec["max_states"],
            spec["search_seconds"],
        )
        temporary = session / "reference-ready.tmp"
        temporary.write_text(json.dumps(references, ensure_ascii=False))
        temporary.replace(session / "reference-ready.json")
    except Exception as error:
        (session / "reference-error").write_text(str(error))
        (log_directory / "reference-error").write_text(str(error))
        raise
