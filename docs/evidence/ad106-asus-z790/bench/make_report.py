"""Build the comparison table and release chart straight from bench JSON.

Usage: python make_report.py <stock.json> <unlocked.json> [<extra.json> ...]
Each file contributes one series, labelled with its "label" field (or the
--names list). Prints a markdown table (best of passes per case, gain vs the
first file) and renders chart.html -> chart.png next to this script with
headless Chrome.
"""

import json
import subprocess
import sys
from pathlib import Path

CASES = [("FP16", 8192), ("BF16", 8192), ("TF32", 4096), ("FP32 (control)", 4096)]
COLORS = ["#3987e5", "#d95926", "#199e70"]  # validated dark categorical slots 1-3
CHROME = r"C:\Program Files\Google\Chrome\Application\chrome.exe"
HERE = Path(__file__).parent


def best(path):
    r = json.loads(Path(path).read_text())
    out = {}
    for row in r["rows"]:
        key = (row["case"], row["n"])
        out[key] = max(out.get(key, 0.0), row["median"])
    return r, out


def main():
    args = sys.argv[1:]
    names = None
    if "--names" in args:
        i = args.index("--names")
        names = args[i + 1].split("|")
        args = args[:i] + args[i + 2:]
    runs = [best(p) for p in args]
    names = names or [r["label"] for r, _ in runs]
    base = runs[0][1]

    print("| TFLOPS | " + " | ".join(names) + " | Gain |")
    print("|---|" + "---:|" * (len(names) + 1))
    for case, n in CASES:
        vals = [b[(case, n)] for _, b in runs]
        gain = vals[-1] / vals[0] - 1
        print(f"| {case} {n}³ | " + " | ".join(f"{v:.2f}" for v in vals) + f" | {gain * 100:+.0f} % |")

    groups = [{"name": f"{c.replace(' (control)', '')}  {n}³" + (" (control)" if "control" in c else ""),
               "v": [b[(c, n)] for _, b in runs]} for c, n in CASES]
    r0 = runs[0][0]
    html = (HERE / "chart_template.html").read_text(encoding="utf-8")
    html = (html.replace("__GROUPS__", json.dumps(groups))
                .replace("__NAMES__", json.dumps(names))
                .replace("__COLORS__", json.dumps(COLORS[:len(names)]))
                .replace("__FOOT__", f"driver {r0['driver']} · PyTorch {r0['torch'].split('+')[0]} · "
                                     f"median of 20, best of 2 passes"))
    out = HERE / "chart.html"
    out.write_text(html, encoding="utf-8")
    png = HERE / "chart.png"
    subprocess.run([CHROME, "--headless=new", "--disable-gpu", "--hide-scrollbars", "--window-size=1600,900",
                    f"--screenshot={png}", out.as_uri()], check=True, capture_output=True)
    print("rendered", png)


if __name__ == "__main__":
    main()
