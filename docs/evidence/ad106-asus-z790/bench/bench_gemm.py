"""Tensor GEMM throughput for a stock vs unlocked A/B on one card.

Same method as gpu-unlock-ada #99/#80: torch.mm on contiguous randn inputs,
FP32 accumulate forced (no reduced-precision reduction), warm-up then timed
iterations, median TFLOPS. FP32 (no TF32) is the control row: the unlock
must leave it unchanged.

Usage: python bench_gemm.py <label>   (writes result-<label>.json next to it)
"""

import json
import statistics
import subprocess
import sys
import time
from pathlib import Path

import torch

WARMUP = 5
TIMED = 20
PASSES = 2


def smi(query):
    out = subprocess.run(
        ["nvidia-smi", f"--query-gpu={query}", "--format=csv,noheader,nounits"],
        capture_output=True, text=True,
    )
    return out.stdout.strip()


def gemm_tflops(n, dtype, tf32=False):
    torch.backends.cuda.matmul.allow_tf32 = tf32
    a = torch.randn(n, n, device="cuda", dtype=dtype)
    b = torch.randn(n, n, device="cuda", dtype=dtype)
    for _ in range(WARMUP):
        torch.mm(a, b)
    torch.cuda.synchronize()
    times = []
    for _ in range(TIMED):
        start = torch.cuda.Event(enable_timing=True)
        end = torch.cuda.Event(enable_timing=True)
        start.record()
        torch.mm(a, b)
        end.record()
        torch.cuda.synchronize()
        times.append(start.elapsed_time(end) / 1000.0)
    flops = 2.0 * n ** 3
    vals = [flops / t / 1e12 for t in times]
    return statistics.median(vals), min(vals), max(vals)


def correctness():
    # FP16/BF16 vs an FP64 reference: the unlock changes issue rate, never results.
    worst = {}
    for name, dtype in (("fp16", torch.float16), ("bf16", torch.bfloat16)):
        a = torch.randn(2048, 2048, device="cuda", dtype=dtype)
        b = torch.randn(2048, 2048, device="cuda", dtype=dtype)
        got = torch.mm(a, b).double()
        ref = torch.mm(a.double(), b.double())
        worst[name] = ((got - ref).abs().max() / ref.abs().max()).item()
    return worst


def main():
    label = sys.argv[1] if len(sys.argv) > 1 else "run"
    if not torch.cuda.is_available():
        sys.exit("CUDA not available to torch")
    torch.backends.cuda.matmul.allow_fp16_reduced_precision_reduction = False
    torch.backends.cuda.matmul.allow_bf16_reduced_precision_reduction = False

    cases = [
        ("FP32 (control)", 4096, torch.float32, False),
        ("TF32", 4096, torch.float32, True),
        ("FP16", 4096, torch.float16, False),
        ("FP16", 8192, torch.float16, False),
        ("BF16", 4096, torch.bfloat16, False),
        ("BF16", 8192, torch.bfloat16, False),
    ]
    result = {
        "label": label,
        "time": time.strftime("%Y-%m-%d %H:%M:%S"),
        "torch": torch.__version__,
        "cuda": torch.version.cuda,
        "device": torch.cuda.get_device_name(0),
        "driver": smi("driver_version"),
        "power_limit_w": smi("power.limit"),
        "rows": [],
    }
    print(f"{result['device']}  torch {result['torch']}  CUDA {result['cuda']}  driver {result['driver']}")
    print(f"{'case':<16}{'N':>6}  {'median':>8}  {'min':>7}  {'max':>7}   (TFLOPS, pass)")
    for p in range(1, PASSES + 1):
        for name, n, dtype, tf32 in cases:
            med, lo, hi = gemm_tflops(n, dtype, tf32)
            row = {"case": name, "n": n, "pass": p, "median": round(med, 2), "min": round(lo, 2), "max": round(hi, 2)}
            # Sample under load: keep this case's GEMM running until the
            # nvidia-smi process (slow to start) has returned its reading.
            torch.backends.cuda.matmul.allow_tf32 = tf32
            a = torch.randn(n, n, device="cuda", dtype=dtype)
            for _ in range(5):
                torch.mm(a, a)
            torch.cuda.synchronize()
            proc = subprocess.Popen(
                ["nvidia-smi", "--query-gpu=clocks.sm,power.draw,clocks_event_reasons.sw_power_cap",
                 "--format=csv,noheader,nounits"],
                stdout=subprocess.PIPE, text=True,
            )
            while proc.poll() is None:
                torch.mm(a, a)
                torch.cuda.synchronize()
            clock, power, cap = proc.stdout.read().strip().split(", ")
            row["clock_mhz"], row["power_w"], row["power_capped"] = clock, power, cap == "Active"
            row["tflops_per_ghz"] = round(med / (float(clock) / 1000.0), 2)
            result["rows"].append(row)
            print(f"{name:<16}{n:>6}  {med:8.2f}  {lo:7.2f}  {hi:7.2f}   p{p}  {clock} MHz {power} W"
                  f"{'  POWER-CAPPED' if row['power_capped'] else ''}  {row['tflops_per_ghz']}/GHz")
    result["max_rel_error"] = correctness()
    print("max relative error vs FP64:", result["max_rel_error"])
    out = Path(__file__).with_name(f"result-{label}.json")
    out.write_text(json.dumps(result, indent=2))
    print("saved", out)


if __name__ == "__main__":
    main()
