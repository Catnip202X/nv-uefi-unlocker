"""Sustained-load stability soak for the unlocked card.

Alternates FP16/BF16 8192^3 GEMMs (FP32 accumulate) for DURATION seconds.
Every CHECK seconds: throughput over the last window, a 2048^3 result check
against FP64, and clock/power/temperature. Any NaN/Inf or an error jump is
a failure. Writes soak-<label>.json next to this file.

Usage: python soak.py <label> [seconds]
"""

import json
import subprocess
import sys
import time
from pathlib import Path

import torch

CHECK = 30
N = 8192


def smi():
    out = subprocess.run(
        ["nvidia-smi", "--query-gpu=clocks.sm,power.draw,temperature.gpu,clocks_event_reasons.hw_slowdown",
         "--format=csv,noheader,nounits"],
        capture_output=True, text=True,
    ).stdout.strip()
    clock, power, temp, hw = out.split(", ")
    return {"clock_mhz": float(clock), "power_w": float(power), "temp_c": float(temp), "hw_slowdown": hw == "Active"}


def check():
    worst = 0.0
    for dtype in (torch.float16, torch.bfloat16):
        a = torch.randn(2048, 2048, device="cuda", dtype=dtype)
        b = torch.randn(2048, 2048, device="cuda", dtype=dtype)
        got = torch.mm(a, b).double()
        if not torch.isfinite(got).all():
            return float("inf")
        ref = torch.mm(a.double(), b.double())
        worst = max(worst, ((got - ref).abs().max() / ref.abs().max()).item())
    return worst


def main():
    label = sys.argv[1] if len(sys.argv) > 1 else "soak"
    duration = float(sys.argv[2]) if len(sys.argv) > 2 else 600.0
    torch.backends.cuda.matmul.allow_fp16_reduced_precision_reduction = False
    torch.backends.cuda.matmul.allow_bf16_reduced_precision_reduction = False
    mats = {
        "fp16": (torch.randn(N, N, device="cuda", dtype=torch.float16),) * 2,
        "bf16": (torch.randn(N, N, device="cuda", dtype=torch.bfloat16),) * 2,
    }
    flops = 2.0 * N ** 3
    windows = []
    failed = False
    t_end = time.time() + duration
    print(f"soak {label}: {duration:.0f}s, FP16/BF16 {N}^3 alternating, check every {CHECK}s", flush=True)
    while time.time() < t_end:
        count = 0
        t0 = time.time()
        while time.time() - t0 < CHECK and time.time() < t_end:
            for a, b in mats.values():
                torch.mm(a, b)
                count += 1
            if count % 8 == 0:
                torch.cuda.synchronize()
        torch.cuda.synchronize()
        elapsed = time.time() - t0
        tel = smi()
        err = check()
        tflops = count * flops / elapsed / 1e12
        ok = err < 0.01
        failed |= not ok
        windows.append({"t": round(duration - (t_end - time.time())), "tflops": round(tflops, 2), "max_rel_err": err, **tel})
        print(f"t={windows[-1]['t']:4d}s  {tflops:6.2f} TFLOPS  err={err:.2e}  {tel['clock_mhz']:.0f} MHz  "
              f"{tel['power_w']:.0f} W  {tel['temp_c']:.0f} C{'  HW-SLOWDOWN' if tel['hw_slowdown'] else ''}"
              f"{'' if ok else '  FAIL'}", flush=True)
    t = [w["tflops"] for w in windows]
    summary = {
        "label": label, "duration_s": duration, "windows": windows, "passed": not failed,
        "tflops_min": min(t), "tflops_max": max(t), "tflops_mean": round(sum(t) / len(t), 2),
        "temp_max_c": max(w["temp_c"] for w in windows),
    }
    Path(__file__).with_name(f"soak-{label}.json").write_text(json.dumps(summary, indent=2))
    print(f"RESULT: {'PASS' if summary['passed'] else 'FAIL'}  mean {summary['tflops_mean']} TFLOPS "
          f"(min {summary['tflops_min']}, max {summary['tflops_max']})  max temp {summary['temp_max_c']:.0f} C", flush=True)


if __name__ == "__main__":
    main()
