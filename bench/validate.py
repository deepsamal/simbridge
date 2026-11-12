"""Check that each task's grader tells right from wrong before any agent sees it.

For every task: the agent's starter workspace must build; the reference solution
must build and PASS; every seeded fault (reference + the fault's overlay files)
must build and FAIL. Any other outcome means the task or its grader is broken.

    python bench/validate.py                     # every task under bench/tasks
    python bench/validate.py bench/tasks/B1-*    # some
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import grading  # noqa: E402

WORK = HERE / "target"  # ignored by git ("target/")


def build(src, task, variant):
    """Copy a crate to a scratch dir, build it for the task's target, return the ELF."""
    dst = WORK / "build" / task["id"] / variant
    if dst.exists():
        shutil.rmtree(dst)
    shutil.copytree(src, dst, ignore=shutil.ignore_patterns("target"))
    # One target dir per variant: every variant has the same package name, so a
    # shared dir could hand back another variant's ELF. Costs a dependency rebuild each.
    tdir = WORK / "cargo" / task["id"] / variant
    env = dict(os.environ, CARGO_TARGET_DIR=str(tdir))
    cmd = ["cargo", "build", "--release", "--quiet", "--target", task["target"]]
    r = subprocess.run(cmd, cwd=dst, env=env, capture_output=True, text=True)
    if r.returncode != 0:
        return None, r.stderr.strip()[-1500:]
    elf = tdir / task["target"] / "release" / task["package"]
    keep = WORK / "elfs" / f"{task['id']}-{variant}.elf"
    keep.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(elf, keep)
    return keep, None


def overlay(reference, fault_dir, task):
    """Reference crate with the fault's files copied over it."""
    dst = WORK / "src" / task["id"] / fault_dir.name
    if dst.exists():
        shutil.rmtree(dst)
    shutil.copytree(reference, dst, ignore=shutil.ignore_patterns("target"))
    for f in fault_dir.rglob("*"):
        if f.is_file() and f.name != "fault.json":
            out = dst / f.relative_to(fault_dir)
            out.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(f, out)
    return dst


def failed_checks(res):
    return [c["name"] for c in res["checks"] if not c["passed"]]


def validate(task_dir, exe):
    task = json.loads((task_dir / "task.json").read_text(encoding="utf-8"))
    rows, ok = [], True

    elf, err = build(task_dir / "workspace", task, "workspace")
    good = elf is not None
    rows.append(("workspace", "builds", "builds" if good else "BUILD FAILED", good, err or ""))
    ok &= good

    ref = task_dir / "hidden" / "reference"
    elf, err = build(ref, task, "reference")
    if elf is None:
        rows.append(("reference", "pass", "BUILD FAILED", False, err))
        return False, rows
    res = grading.run(task_dir, elf, exe)
    good = res["passed"]
    rows.append(("reference", "pass", f"pass ({res['score']})" if good else f"FAIL ({res['score']})", good,
                 res["error"] or ", ".join(failed_checks(res))))
    ok &= good

    for fault in sorted(p for p in (task_dir / "hidden" / "faults").glob("*") if p.is_dir()):
        elf, err = build(overlay(ref, fault, task), task, fault.name)
        if elf is None:
            rows.append((fault.name, "fail", "BUILD FAILED", False, err))
            ok = False
            continue
        res = grading.run(task_dir, elf, exe)
        good = not res["passed"]
        rows.append((fault.name, "fail", f"fail ({res['score']})" if good else "PASSED (grader misses it)", good,
                     res["error"] or "failed: " + ", ".join(failed_checks(res))))
        ok &= good
    return ok, rows


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("tasks", nargs="*", help="task directories (default: all)")
    ap.add_argument("--simbridge", help="simbridge executable")
    a = ap.parse_args()
    tasks = [Path(t) for t in a.tasks] or sorted(p for p in (HERE / "tasks").glob("*") if p.is_dir())
    all_ok = True
    for t in tasks:
        ok, rows = validate(t.resolve(), a.simbridge)
        all_ok &= ok
        print(f"\n=== {t.name}: {'VALID' if ok else 'INVALID'}")
        for variant, expect, got, good, note in rows:
            print(f"  {'ok ' if good else 'BAD'} {variant:<28} expect {expect:<7} got {got:<26} {note}")
    print("\nall tasks valid" if all_ok else "\nsome tasks are invalid")
    sys.exit(0 if all_ok else 1)


if __name__ == "__main__":
    main()
