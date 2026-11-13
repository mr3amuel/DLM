"""Generate the native accent catalog from the same tokens used by SwiftUI."""
import json
import re
import sys
from pathlib import Path

if len(sys.argv) != 3:
    raise SystemExit("Usage: export_brand_colors.py Design.swift output.xcassets")
design, output = map(Path, sys.argv[1:])
source = design.read_text()
endpoints = []
for token in ("brandTurquoiseHex", "brandBlueHex"):
    match = re.search(rf"{token}: UInt32 = 0x([0-9A-Fa-f]{{6}})", source)
    if not match:
        raise SystemExit(f"Missing authoritative brand token: {token}")
    endpoints.append(match.group(1).upper())
output.mkdir(parents=True, exist_ok=True)
info = {"author": "xcode", "version": 1}
(output / "Contents.json").write_text(json.dumps({"info": info}))
ui_token = re.search(r"uiAccentHex: UInt32 = (brandTurquoiseHex|brandBlueHex)", source)
if not ui_token:
    raise SystemExit("UI accent must reference an existing original logo color.")
ui_color = dict(zip(("brandTurquoiseHex", "brandBlueHex"), endpoints))[ui_token.group(1)]
colors = dict(zip(("DLMBrandPrimary", "DLMBrandSecondary"), endpoints))
colors["DLMUIAccent"] = ui_color
for name, color in colors.items():
    folder = output / f"{name}.colorset"
    folder.mkdir(exist_ok=True)
    components = {key: str(int(color[i:i + 2], 16) / 255) for key, i in (("red", 0), ("green", 2), ("blue", 4))}
    components["alpha"] = "1.0"
    (folder / "Contents.json").write_text(json.dumps({
        "info": info, "colors": [{"idiom": "universal", "color": {"color-space": "srgb", "components": components}}]
    }, indent=2))
print("Generated native accent catalog from the validated original DLM brand tokens.")
