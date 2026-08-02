#!/usr/bin/env python3
"""Scan all neo sessions for DelegateSwarm data and analyze real progress curves.

Read-only analysis: extracts DelegateSwarmStarted/Updated/ProgressUpdated/Finished
events from every session's wire.jsonl, reconstructs per-swarm timelines, and
prints statistics useful for calibrating the Bayesian progress estimator.
"""
import json
import glob
import os
from collections import defaultdict

SESSIONS = os.path.expanduser("~/.neo/sessions")

def iter_wire_files():
    for wd in glob.glob(os.path.join(SESSIONS, "wd_*")):
        for wire in glob.glob(os.path.join(wd, "session_*", "agents", "*", "wire.jsonl")):
            yield wire

def parse_events():
    swarms = {}
    n_events = defaultdict(int)
    files_scanned = 0
    for wire in iter_wire_files():
        files_scanned += 1
        try:
            with open(wire, "r", errors="replace") as f:
                lines = f.readlines()
        except OSError:
            continue
        for line in lines:
            line = line.strip()
            if not line or '"DelegateSwarm' not in line and '"Delegate' not in line:
                continue
            try:
                ev = json.loads(line)
            except json.JSONDecodeError:
                continue
            for key, payload in ev.items():
                if key in ("DelegateSwarmStarted", "DelegateSwarmUpdated", "DelegateSwarmFinished",
                           "DelegateSwarmProgressUpdated", "DelegateStarted", "DelegateFinished"):
                    n_events[key] += 1
                if key in ("DelegateSwarmStarted", "DelegateSwarmUpdated", "DelegateSwarmFinished"):
                    swarm = payload.get("swarm", {})
                    sid = swarm.get("swarm_id")
                    rec = swarms.setdefault(sid, {"start": None, "finish": None, "events": []})
                    rec["events"].append((key, swarm))
                    if key == "DelegateSwarmStarted":
                        rec["start"] = swarm
                    if key == "DelegateSwarmFinished":
                        rec["finish"] = swarm
                elif key == "DelegateSwarmProgressUpdated":
                    sid = payload.get("swarm_id")
                    swarms.setdefault(sid, {"start": None, "finish": None, "events": []})
                    swarms[sid]["events"].append((key, payload))
    return swarms, n_events, files_scanned

def main():
    swarms, n_events, files_scanned = parse_events()
    print(f"files scanned: {files_scanned}")
    print(f"event counts: {dict(n_events)}")
    print(f"swarms found: {len(swarms)}")
    print()

    all_durs = []
    per_swarm = []
    for sid, rec in swarms.items():
        start = rec["start"]
        finish = rec["finish"]
        if not start or not finish:
            per_swarm.append((sid, "no-finish", 0, 0, []))
            continue
        agg_f = finish.get("aggregate", {})
        n_children = agg_f.get("total", 0)
        durs = []
        finish_children = {c["agent"]["id"]: c["agent"] for c in finish.get("children", [])}
        for cid, agent in finish_children.items():
            st = agent.get("started_at_ms")
            te = agent.get("terminal_at_ms")
            if st and te:
                durs.append(te - st)
        all_durs.extend(durs)
        per_swarm.append((sid, agg_f.get("status", "?"), n_children, len(durs), sorted(durs)))

    print("=== per-swarm summary (id, final status, total children, sampled count, sorted durations ms) ===")
    for sid, status, n, sampled, durs in sorted(per_swarm, key=lambda x: x[3], reverse=True):
        durs_s = ", ".join(str(d) for d in durs[:12])
        print(f"{sid} status={status} children={n} sampled={sampled} durs=[{durs_s}]")

    if all_durs:
        all_durs.sort()
        n = len(all_durs)
        def pct(p):
            return all_durs[min(n - 1, int(n * p))]
        print()
        print(f"=== child duration distribution (ms) — n={n} ===")
        print(f"min={all_durs[0]} p10={pct(0.1)} p25={pct(0.25)} median={pct(0.5)} p75={pct(0.75)} p90={pct(0.9)} max={all_durs[-1]}")

    print()
    print("=== swarm progress timelines (completed, running, queued) ===")
    for sid, rec in swarms.items():
        timeline = []
        for key, data in rec["events"]:
            if key == "DelegateSwarmProgressUpdated":
                agg = data.get("aggregate", {})
                timeline.append((agg.get("completed", 0), agg.get("running", 0), agg.get("queued", 0)))
        if timeline:
            first, last = timeline[0], timeline[-1]
            print(f"{sid}: {len(timeline)} progress events; first agg={first} last agg={last}")

if __name__ == "__main__":
    main()
