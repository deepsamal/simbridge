"""Check that each task's grader tells right from wrong before any agent sees it.

For every task: the agent's starter workspace must build; the reference solution
must build and PASS; every seeded fault (reference + the fault's overlay files)
must build and FAIL. Any other outcome means the task or its grader is broken.

    python bench/validate.py                     # every task under bench/tasks
    python bench/validate.py bench/tasks/B1-*    # some

Building and grading can run on different machines (e.g. cargo on one, Renode
on another): `--build-only` builds every variant into bench/target/elfs, and
`--prebuilt DIR` grades the ELFs found in DIR instead of building. Extra cargo
arguments (e.g. -Zbuild-std=core) can be passed in VALIDATE_CARGO_ARGS.
"""

import argparse
import json
import os
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import grading  # noqa: E402

WORK = HERE / "target"  # ignored by git ("target/")
PREBUILT = None  # set by --prebuilt: grade these ELFs, build nothing
BUILD_ONLY = False  # set by --build-only: build every variant, grade nothing


def build(src, task, variant):
    """Copy a crate to a scratch dir, build it for the task's target, return the ELF."""
    if PREBUILT:
        elf = PREBUILT / f"{task['id']}-{variant}.elf"
        return (elf, None) if elf.is_file() else (None, f"no prebuilt {elf.name}")
    dst = WORK / "build" / task["id"] / variant
    if dst.exists():
        shutil.rmtree(dst)
    shutil.copytree(src, dst, ignore=shutil.ignore_patterns("target"))
    # One target dir per variant: every variant has the same package name, so a
    # shared dir could hand back another variant's ELF. Costs a dependency rebuild each.
    tdir = WORK / "cargo" / task["id"] / variant
    env = dict(os.environ, CARGO_TARGET_DIR=str(tdir))
    cmd = ["cargo", "build", "--release", "--quiet", "--target", task["target"]]
    cmd += shlex.split(os.environ.get("VALIDATE_CARGO_ARGS", ""))
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
    if PREBUILT:
        return fault_dir  # only its name is used: the ELF is already built
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


def grade(task_dir, elf, exe):
    if BUILD_ONLY:
        return {"passed": None, "score": "-", "error": None, "checks": []}
    return grading.run(task_dir, elf, exe)


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
    res = grade(task_dir, elf, exe)
    good = res["passed"] is not False
    rows.append(("reference", "pass", "built" if BUILD_ONLY else f"pass ({res['score']})" if good else f"FAIL ({res['score']})", good,
                 res["error"] or ", ".join(failed_checks(res))))
    ok &= good

    for fault in sorted(p for p in (task_dir / "hidden" / "faults").glob("*") if p.is_dir()):
        elf, err = build(overlay(ref, fault, task), task, fault.name)
        if elf is None:
            rows.append((fault.name, "fail", "BUILD FAILED", False, err))
            ok = False
            continue
        res = grade(task_dir, elf, exe)
        good = res["passed"] is not True
        rows.append((fault.name, "fail", "built" if BUILD_ONLY else f"fail ({res['score']})" if good else "PASSED (grader misses it)", good,
                     res["error"] or ("" if BUILD_ONLY else "failed: " + ", ".join(failed_checks(res)))))
        ok &= good
    return ok, rows


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("tasks", nargs="*", help="task directories (default: all)")
    ap.add_argument("--simbridge", help="simbridge executable")
    g = ap.add_mutually_exclusive_group()
    g.add_argument("--build-only", action="store_true", help="build every variant into bench/target/elfs; no grading")
    g.add_argument("--prebuilt", metavar="DIR", help="grade the ELFs in DIR (named <task>-<variant>.elf); no building")
    a = ap.parse_args()
    global PREBUILT, BUILD_ONLY
    BUILD_ONLY = a.build_only
    PREBUILT = Path(a.prebuilt).resolve() if a.prebuilt else None
    tasks = [Path(t) for t in a.tasks] or sorted(p for p in (HERE / "tasks").glob("*") if p.is_dir())
    all_ok = True
    for t in tasks:
        ok, rows = validate(t.resolve(), a.simbridge)
        all_ok &= ok
        status = ('BUILT' if ok else 'BUILD FAILED') if BUILD_ONLY else ('VALID' if ok else 'INVALID')
        print(f"\n=== {t.name}: {status}")
        for variant, expect, got, good, note in rows:
            print(f"  {'ok ' if good else 'BAD'} {variant:<28} expect {expect:<7} got {got:<26} {note}")
    if BUILD_ONLY:
        print(f"\nELFs in {WORK / 'elfs'}" if all_ok else "\nsome builds failed")
    else:
        print("\nall tasks valid" if all_ok else "\nsome tasks are invalid")
    sys.exit(0 if all_ok else 1)


if __name__ == "__main__":
    main()
