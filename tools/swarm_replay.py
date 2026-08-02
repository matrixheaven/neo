#!/usr/bin/env python3
"""Replay the SwarmProgressEstimator against real swarm timelines from neo sessions.

Replicates the estimator logic from crates/neo-agent-core/src/multi_agent/progress.rs
(time-credit log-normal CDF + tool credit + confidence-weighted aggregate) and
grid-searches estimator config against real data to minimize over-estimation.

Read-only: only reads ~/.neo/sessions.
"""
import json
import glob
import os
import math
from collections import defaultdict

SESSIONS = os.path.expanduser("~/.neo/sessions")

def iter_wire_files():
    for wd in glob.glob(os.path.join(SESSIONS, "wd_*")):
        for wire in glob.glob(os.path.join(wd, "session_*", "agents", "*", "wire.jsonl")):
            yield wire

def load_swarm_timelines():
    """Return {swarm_id: {'start': {item_index: agent}, 'events': [(ts, aggregate, child), ...]}}."""
    swarms = defaultdict(lambda: {"start": {}, "events": []})
    for wire in iter_wire_files():
        try:
            lines = open(wire, errors="replace").readlines()
        except OSError:
            continue
        for line in lines:
            if '"DelegateSwarm' not in line:
                continue
            try:
                ev = json.loads(line)
            except json.JSONDecodeError:
                continue
            if "DelegateSwarmStarted" in ev:
                payload = ev["DelegateSwarmStarted"]
                swarm = payload.get("swarm", {})
                sid = swarm.get("swarm_id")
                if not sid:
                    continue
                for child in swarm.get("children", []):
                    item = child.get("item_index")
                    if item is not None:
                        swarms[sid]["start"][item] = child.get("agent", {})
                continue
            payload = ev.get("DelegateSwarmProgressUpdated")
            if not payload:
                continue
            sid = payload.get("swarm_id")
            if not sid:
                continue
            agg = payload.get("aggregate", {})
            cp = payload.get("child_progress", {})
            if not isinstance(cp, dict) or "progress" not in cp:
                continue
            child = cp["progress"]
            item_index = cp.get("item_index")
            ts = child.get("updated_at_ms", 0)
            if not ts:
                continue
            swarms[sid]["events"].append((ts, agg, item_index, child))
    for sid in swarms:
        swarms[sid]["events"].sort(key=lambda x: x[0])
    return dict(swarms)

# ---- estimator replica (mirrors progress.rs) ----

def lognormal_cdf(x, median, sigma):
    if x <= 0:
        return 0.0
    sigma = max(sigma, 0.01)
    z = math.log(x / max(median, 1.0)) / (sigma * math.sqrt(2))
    return 0.5 * (1.0 + math.erf(z))

class Estimator:
    def __init__(self, cfg):
        self.cfg = cfg
        self.members = {}  # id -> dict
        self.samples = []

    def _ensure(self, mid, now):
        if mid not in self.members:
            self.members[mid] = {"started": None, "terminal": None, "last_activity": now,
                                 "tools": set(), "display": 0.0}

    def started(self, mid, ts):
        self._ensure(mid, ts)
        m = self.members[mid]
        m["started"] = m["started"] or ts
        m["last_activity"] = max(m["last_activity"] or 0, ts)

    def activity(self, mid, ts):
        self._ensure(mid, ts)
        m = self.members[mid]
        m["last_activity"] = max(m["last_activity"] or 0, ts)

    def tool(self, mid, tid, ts):
        self._ensure(mid, ts)
        m = self.members[mid]
        m["started"] = m["started"] or ts
        if tid not in m["tools"]:
            m["tools"].add(tid)
            m["last_activity"] = max(m["last_activity"] or 0, ts)
            m["display"] = max(m["display"], self.cfg["initial_tool_credit_floor"])

    def terminal(self, mid, ts, sample=True):
        self._ensure(mid, ts)
        m = self.members[mid]
        already = m["terminal"] is not None
        m["terminal"] = m["terminal"] or ts
        m["last_activity"] = ts
        m["display"] = max(m["display"], 1.0)
        if sample and not already and m["started"]:
            self.samples.append(max(ts - m["started"], 1))

    def prior_duration(self):
        if not self.samples:
            return self.cfg["cold_start_prior_ms"], self.cfg["prior_shape"]
        s = sorted(self.samples)
        med = s[len(s) // 2]
        return max(med * self.cfg["workload_spread_factor"], 1.0), self.cfg["prior_shape"]

    def estimate(self, mid, phase, capacity, now):
        """phase in {queued, running, completed, failed, cancelled}; returns (progress, confidence)"""
        self._ensure(mid, now)
        m = self.members[mid]
        if phase in ("completed", "failed", "cancelled"):
            m["display"] = max(m["display"], max(capacity, 1.0))
            return 1.0, 1.0
        if m["started"] is None:
            return 0.0, 0.0
        last = m["last_activity"]
        eff_now = min(now, last + self.cfg["stale_activity_after_ms"]) if last else now
        elapsed = max(eff_now - m["started"], 0)
        prior, shape = self.prior_duration()
        tc = lognormal_cdf(elapsed, prior, shape)
        tools = len(m["tools"])
        tool_cap = self.cfg.get("tool_credit_cap", 0.35)
        tool_credit = min(0.15 * math.log(1.0 + tools), tool_cap)
        combined = min(tc + tool_credit, self.cfg["unfinished_progress_cap"])
        ticks = max(capacity * combined, m["display"])
        m["display"] = ticks
        prog = min(ticks / capacity, self.cfg["aggregate_progress_cap"]) if capacity > 0 else 0.0
        conf = self.cfg["min_running_weight"] + (1.0 - self.cfg["min_running_weight"]) * tc
        return prog, conf

    def weighted_progress(self, children, now):
        """children: [(phase, mid)]"""
        total = 0.0
        wsum = 0.0
        for mid, phase in children:
            p, c = self.estimate(mid, phase, 1.0, now)
            total += p * c
            wsum += 1.0
        return min(total / wsum, 0.95) if wsum else 1.0

# ---- replay ----

PHASES = {"queued": "queued", "running": "running", "completed": "completed",
          "failed": "failed", "cancelled": "cancelled", "interrupted": "cancelled",
          "timed_out": "failed"}

def replay(swarm, cfg):
    """Return list of (elapsed_since_start_s, estimate, true_progress)."""
    est = Estimator(cfg)
    tool_counts = defaultdict(int)
    out = []
    t0 = None
    # current per-item child state, seeded from DelegateSwarmStarted
    current = {}
    for item_idx, agent in swarm["start"].items():
        current[item_idx] = {
            "agent_id": agent.get("id", f"item_{item_idx}"),
            "state": agent.get("state", "queued"),
            "started_at_ms": agent.get("started_at_ms"),
            "terminal_at_ms": agent.get("terminal_at_ms"),
            "updated_at_ms": agent.get("updated_at_ms", 0),
            "tool_count": agent.get("tool_count", 0),
        }
    for ts, agg, item_idx, child in swarm["events"]:
        if t0 is None:
            t0 = ts
        state = child.get("state", "queued")
        if item_idx is not None:
            prev = current.get(item_idx, {})
            merged = dict(prev)
            merged["agent_id"] = child.get("agent_id", f"item_{item_idx}")
            merged["state"] = state
            # started/terminal timestamps only appear on transition events;
            # never overwrite a known value with None
            if child.get("started_at_ms") is not None:
                merged["started_at_ms"] = child["started_at_ms"]
            if child.get("terminal_at_ms") is not None:
                merged["terminal_at_ms"] = child["terminal_at_ms"]
            merged["updated_at_ms"] = max(child.get("updated_at_ms", ts), prev.get("updated_at_ms", 0))
            merged["tool_count"] = max(child.get("tool_count", 0), prev.get("tool_count", 0))
            current[item_idx] = merged
        # sync estimator
        children = []
        for item_idx, c in current.items():
            mid = c["agent_id"]
            st = c["started_at_ms"]
            te = c["terminal_at_ms"]
            if st is not None:
                est.started(mid, st)
            if te is not None:
                est.terminal(mid, te, sample=(c["state"] in ("completed", "failed", "timed_out")))
            est.activity(mid, c["updated_at_ms"])
            # tool ids are not recorded in progress snapshots; emulate via count increments
            n = c["tool_count"]
            prev = tool_counts.get(mid, 0)
            if n > prev:
                tool_counts[mid] = n
                for i in range(prev, n):
                    est.tool(mid, f"{mid}#{i}", c["updated_at_ms"])
            children.append((mid, PHASES.get(c["state"], "queued")))
        estv = est.weighted_progress(children, ts)
        total = agg.get("total", 0)
        true = agg.get("completed", 0) / total if total else 0.0
        out.append(((ts - t0) / 1000.0, estv, true))
    return out

DEFAULT_CFG = {
    "unfinished_progress_cap": 0.7,
    "aggregate_progress_cap": 0.95,
    "min_running_weight": 0.1,
    "cold_start_prior_ms": 600_000.0,
    "prior_shape": 0.5,
    "workload_spread_factor": 3.0,
    "tool_credit_cap": 0.2,
    "initial_tool_credit_floor": 0.12,
    "stale_activity_after_ms": 45_000,
}

def is_real_swarm(swarm):
    """A real swarm has at least one child that ran > 60s and a sane event count.
    Excludes pathological swarms (e.g. token-stream spam with 80k events)."""
    if len(swarm["events"]) > 5000:
        return False
    durs = []
    for ts, agg, item_idx, child in swarm["events"]:
        st = child.get("started_at_ms")
        te = child.get("terminal_at_ms")
        if st and te:
            durs.append(te - st)
    return (len(swarm["events"]) > 100) or any(d > 60_000 for d in durs)

def evaluate(swarms, cfg, verbose=False):
    """Return (mae, mean_over, early_over, mid_over, n_points).
    early_over = mean over-estimate when true < 0.5 (the 'inflates too fast' complaint is
    mainly visible in the middle/late phase); mid_over = mean over-estimate when true >= 0.5."""
    errs = []
    over = []
    early_over = []
    mid_over = []
    for sid, swarm in swarms.items():
        if not is_real_swarm(swarm):
            continue
        pts = replay(swarm, cfg)
        if verbose:
            # downsample curve for inspection
            step = max(1, len(pts) // 12)
            print(f"  {sid}: " + " ".join(f"{t/60:.0f}m:{estv:.0%}/{true:.0%}" for t, estv, true in pts[::step]))
        for t, estv, true in pts:
            errs.append(abs(estv - true))
            over.append(estv - true)
            if true < 0.5:
                early_over.append(estv - true)
            else:
                mid_over.append(estv - true)
    if not errs:
        return None
    return (sum(errs) / len(errs), sum(over) / len(over),
            sum(early_over) / len(early_over) if early_over else 0.0,
            sum(mid_over) / len(mid_over) if mid_over else 0.0, len(errs))

def main():
    swarms = load_swarm_timelines()
    real = {sid: swarm for sid, swarm in swarms.items() if is_real_swarm(swarm)}
    print(f"swarms: {len(swarms)} total, {len(real)} real (child>60s, <=5000 events)")
    for sid in sorted(real, key=lambda s: -len(real[s]["events"]))[:40]:
        print(f"  {sid}: {len(real[sid]['events'])} events")
    print()

    base = evaluate(real, DEFAULT_CFG)
    print(f"baseline (current defaults): MAE={base[0]:.3f} mean_over={base[1]:+.3f} "
          f"early_over(t<0.5)={base[2]:+.3f} mid_over(t>=0.5)={base[3]:+.3f} points={base[4]}")
    print()

    print("=== per-swarm curves (baseline): elapsed:est/true ===")
    evaluate(real, DEFAULT_CFG, verbose=True)
    print()

    # grid search, constrained to parameter ranges justified by the real data:
    # - real child durations cluster around 3-20 min (median ~6-7 min, heavy tail to 40min+)
    # - worst/median duration ratio within a swarm is ~1.3-2.5 (excluding stuck agents)
    # - log-space spread of real durations ≈ 0.6-0.7
    candidates = []
    for cold in (300_000, 420_000, 600_000):
        for shape in (0.5, 0.6, 0.7):
            for spread in (2.0, 2.5, 3.0):
                for minw in (0.1, 0.15, 0.2):
                    for tool_cap in (0.2, 0.25, 0.35):
                        for ucap in (0.7, 0.75, 0.8):
                            cfg = dict(DEFAULT_CFG, cold_start_prior_ms=cold, prior_shape=shape,
                                       workload_spread_factor=spread, min_running_weight=minw,
                                       tool_credit_cap=tool_cap, unfinished_progress_cap=ucap)
                            r = evaluate(real, cfg)
                            if r is None:
                                continue
                            mae, mo, eo, mid, n = r
                            # objective: penalize middle/late over-estimation heavily (the user's
                            # complaint: the bar races ahead while the last agents still run);
                            # also avoid pathological under-shooting (early < -0.15 is "stuck")
                            score = mae + max(mid, 0.0) * 2.0 + max(eo, 0.0) * 0.5 + max(-eo - 0.15, 0.0)
                            candidates.append((score, mae, mo, eo, mid, cfg))
    candidates.sort(key=lambda c: c[0])
    print("top 15 configs by score = MAE + 2*max(mid_over,0) + 0.5*max(early_over,0) + under-shoot penalty:")
    for score, mae, mo, eo, mid, cfg in candidates[:15]:
        print(f"  score={score:.3f} MAE={mae:.3f} mean_over={mo:+.3f} early={eo:+.3f} mid={mid:+.3f} "
              f"cold={cfg['cold_start_prior_ms']/1000:.0f}s shape={cfg['prior_shape']} "
              f"spread={cfg['workload_spread_factor']} minw={cfg['min_running_weight']} "
              f"toolcap={cfg.get('tool_credit_cap')} ucap={cfg['unfinished_progress_cap']}")
    print()
    # show the best config's curves on the two most telling swarms (the user's current one
    # and the long-running one) to sanity-check the trade-off
    if candidates:
        best_cfg = candidates[0][5]
        print("=== best config curves: swarm_d49361cda20a48efb0c5dd56d4248b57 (user's swarm) ===")
        evaluate({k: v for k, v in real.items() if k == "swarm_d49361cda20a48efb0c5dd56d4248b57"}, best_cfg, verbose=True)
        print("=== best config curves: swarm_f4c5e4c3334a4fd0a69e5e35e907876f (long-running) ===")
        evaluate({k: v for k, v in real.items() if k == "swarm_f4c5e4c3334a4fd0a69e5e35e907876f"}, best_cfg, verbose=True)

if __name__ == "__main__":
    main()
