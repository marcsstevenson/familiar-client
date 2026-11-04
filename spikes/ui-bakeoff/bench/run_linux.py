#!/usr/bin/env python3
"""WP 0.4 bake-off harness for Linux (works headless under Xvfb).

For each app it launches the release binary several times in bench mode,
waits for the app's own scroll/select/open run to finish, then sends real
key presses with xdotool, measures idle memory (PSS of the whole process
tree, so WebKit's helper processes count) and idle CPU, and takes a
screenshot. Results go to bench/results/linux-<app>.json and a Markdown
summary is printed.

Usage: bench/run_linux.py [--runs 5] [--display :99] [--no-xvfb] gpui gpui-webview tauri
"""
import argparse, json, os, re, shutil, statistics, subprocess, sys, time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
TARGET = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "release"
APPS = {"gpui": TARGET / "bakeoff-gpui", "gpui-webview": TARGET / "bakeoff-gpui", "tauri": TARGET / "bakeoff-tauri"}
APP_ENV = {"gpui": {"FAMILIAR_GPUI_HTML": "native"}, "gpui-webview": {"FAMILIAR_GPUI_HTML": "webview"}, "tauri": {}}
WINDOW_NAME = {"gpui": "Familiar (GPUI)", "gpui-webview": "Familiar (GPUI)", "tauri": "Familiar (Tauri)"}


def pct(xs, p):
    if not xs:
        return None
    xs = sorted(xs)
    k = (len(xs) - 1) * p / 100
    f, c = int(k), min(int(k) + 1, len(xs) - 1)
    return xs[f] + (xs[c] - xs[f]) * (k - f)


def tree(pid):
    kids = {}
    for d in Path("/proc").iterdir():
        if d.name.isdigit():
            try:
                ppid = int((d / "stat").read_text().rsplit(")", 1)[1].split()[1])
                kids.setdefault(ppid, []).append(int(d.name))
            except (OSError, IndexError, ValueError):
                pass
    out, todo = [], [pid]
    while todo:
        p = todo.pop()
        out.append(p)
        todo.extend(kids.get(p, []))
    return out


def pss_mb(pids):
    total = 0
    for p in pids:
        try:
            for line in Path(f"/proc/{p}/smaps_rollup").read_text().splitlines():
                if line.startswith("Pss:"):
                    total += int(line.split()[1])
        except OSError:
            pass
    return total / 1024


def cpu_ticks(pids):
    t = 0
    for p in pids:
        try:
            f = Path(f"/proc/{p}/stat").read_text().rsplit(")", 1)[1].split()
            t += int(f[11]) + int(f[12])
        except (OSError, IndexError):
            pass
    return t


def events(path):
    if not path.exists():
        return []
    out = []
    for l in path.read_text().splitlines():
        try:
            out.append(json.loads(l))
        except json.JSONDecodeError:
            pass  # a line still being written
    return out


def wait_for(path, name, timeout):
    end = time.time() + timeout
    while time.time() < end:
        if any(e["event"] == name for e in events(path)):
            return True
        time.sleep(0.05)
    return False


def run_once(app, run, env, out_dir, screenshot):
    log = out_dir / f"{app}-run{run}.jsonl"
    log.unlink(missing_ok=True)
    env = dict(env, FAMILIAR_BENCH_OUT=str(log), **APP_ENV[app])
    t0 = time.time_ns()
    proc = subprocess.Popen([str(APPS[app])], env=env, stdout=subprocess.DEVNULL, stderr=open(out_dir / f"{app}-run{run}.stderr", "w"))
    res = {"run": run}
    try:
        if not wait_for(log, "first_frame", 60):
            raise RuntimeError("no first frame")
        ev = events(log)
        res["cold_start_ms"] = (int(next(e for e in ev if e["event"] == "first_frame")["t_ns"]) - t0) / 1e6
        if not wait_for(log, "bench_done", 180):
            raise RuntimeError("bench did not finish")
        # Real key presses through the X server.
        wid = subprocess.run(["xdotool", "search", "--name", re.escape(WINDOW_NAME[app])], env=env, capture_output=True, text=True).stdout.split()
        sent = []
        if wid:
            subprocess.run(["xdotool", "windowfocus", "--sync", wid[0]], env=env)
            time.sleep(0.3)
            for _ in range(30):
                sent.append(time.time_ns())
                subprocess.run(["xdotool", "key", "j"], env=env)
                time.sleep(0.15)
            time.sleep(0.5)
        # Idle: settle (WebKit keeps painting for several seconds after the
        # scroll run), then sample memory and CPU.
        time.sleep(12)
        pids = tree(proc.pid)
        res["processes"] = len(pids)
        res["idle_pss_mb"] = pss_mb(pids)
        c0 = cpu_ticks(pids); time.sleep(5); c1 = cpu_ticks(pids)
        res["idle_cpu_pct"] = (c1 - c0) / os.sysconf("SC_CLK_TCK") / 5 * 100
        if screenshot and shutil.which("import"):
            subprocess.run(["import", "-window", "root", str(out_dir / f"{app}.png")], env=env)
        ev = events(log)
        res["scroll_frame_ms"] = next((e["frame_ms"] for e in ev if e["event"] == "scroll"), [])
        res["blank_frames"] = next((e.get("blank_frames") for e in ev if e["event"] == "scroll"), None)
        res["self_key_ms"] = [e["latency_ms"] for e in ev if e["event"] == "self_key"]
        res["open_ms"] = [e["latency_ms"] for e in ev if e["event"] == "open"]
        res["sanitize_ms"] = [e["sanitize_ms"] for e in ev if e["event"] == "open"]
        keys = [e for e in ev if e["event"] == "key"]
        res["key_in_app_ms"] = [e["latency_ms"] for e in keys]
        # External: xdotool send time to the painted frame.
        res["key_external_ms"] = [
            (int(k["t_ns"]) - s) / 1e6 for k, s in zip(keys, sent)
        ] if len(keys) == len(sent) else []
        res["keys_sent"], res["keys_seen"] = len(sent), len(keys)
    finally:
        for p in reversed(tree(proc.pid)):
            try:
                os.kill(p, 15)
            except OSError:
                pass
        proc.wait(timeout=10)
    return res


def summarise(app, runs):
    allf = [f for r in runs for f in r["scroll_frame_ms"]]
    def s(key):
        xs = [x for r in runs for x in r.get(key, [])]
        return {"p50": pct(xs, 50), "p95": pct(xs, 95), "max": max(xs) if xs else None, "n": len(xs)}
    return {
        "app": app,
        "runs": len(runs),
        "cold_start_ms": {"median": statistics.median(r["cold_start_ms"] for r in runs), "p95": pct([r["cold_start_ms"] for r in runs], 95)},
        "idle_pss_mb": statistics.median(r["idle_pss_mb"] for r in runs),
        "idle_cpu_pct": statistics.median(r["idle_cpu_pct"] for r in runs),
        "processes": runs[0]["processes"],
        "scroll": {"p50": pct(allf, 50), "p95": pct(allf, 95), "p99": pct(allf, 99), "max": max(allf) if allf else None,
                    "over_16_7ms_pct": 100 * sum(f > 16.7 for f in allf) / max(1, len(allf)),
                    "over_33ms_pct": 100 * sum(f > 33.4 for f in allf) / max(1, len(allf)),
                    "blank_frames": [r["blank_frames"] for r in runs]},
        "self_key_ms": s("self_key_ms"),
        "key_in_app_ms": s("key_in_app_ms"),
        "key_external_ms": s("key_external_ms"),
        "open_ms": s("open_ms"),
        "sanitize_ms": s("sanitize_ms"),
        "binary_mb": APPS[app].stat().st_size / 1e6,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("apps", nargs="+", choices=list(APPS))
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--display", default=":99")
    ap.add_argument("--no-xvfb", action="store_true")
    a = ap.parse_args()
    out_dir = HERE / "results"
    out_dir.mkdir(exist_ok=True)
    env = dict(os.environ)
    xvfb = None
    if not a.no_xvfb:
        xvfb = subprocess.Popen(["Xvfb", a.display, "-screen", "0", "1280x800x24", "-nolisten", "tcp"], stderr=subprocess.DEVNULL)
        env["DISPLAY"] = a.display
        env.pop("WAYLAND_DISPLAY", None)
        time.sleep(1)
    try:
        for app in a.apps:
            runs = []
            for i in range(a.runs):
                r = run_once(app, i, env, out_dir, screenshot=(i == a.runs - 1))
                print(f"{app} run {i}: start {r['cold_start_ms']:.0f} ms, PSS {r['idle_pss_mb']:.0f} MB", file=sys.stderr)
                runs.append(r)
            summary = summarise(app, runs)
            (out_dir / f"linux-{app}.json").write_text(json.dumps({"summary": summary, "runs": runs}, indent=1))
            print(json.dumps(summary, indent=1))
    finally:
        if xvfb:
            xvfb.terminate()


if __name__ == "__main__":
    main()
