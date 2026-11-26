#!/usr/bin/env python3
"""Writes apps/mac/Tacie/Resources/ThirdPartyNotices.txt, the license notices that ship inside the app (docs/41 8절).

Parts, in order:
  1. notices/components.txt: models and libraries written up by hand (whisper.cpp, Whisper, Silero, 3D-Speaker).
  2. COMPONENTS below: native libraries built or downloaded into the engine, with their texts from notices/texts/.
  3. The Swift packages the app links (Sparkle, posthog-ios), read from Xcode's package checkouts.
  4. Every Rust crate in the shipped engine and MCP helper (`cargo tree`, normal dependencies, Apple silicon),
     each with the license files its package carries. Identical texts are printed once, after the crates that use it.

  --check   rebuild in memory and fail if the committed file differs (release.sh runs it).

Needs cargo, and the app built once (build.sh) so Xcode has checked the Swift packages out.
"""
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
MAC = ROOT / "apps" / "mac"
ENGINE = ROOT / "engine"
NOTICES = MAC / "notices"
OUT = MAC / "Tacie" / "Resources" / "ThirdPartyNotices.txt"
RULE = "=" * 80

# name, where it comes from, how Tacie uses it, text files in notices/texts
COMPONENTS = [
    ("ONNX Runtime  (Microsoft)", "https://onnxruntime.ai",
     "Linked into the search engine to run the bge-m3 and speaker models. Its own third-party notices follow its license.",
     ["onnxruntime-LICENSE.txt", "onnxruntime-ThirdPartyNotices.txt"]),
    ("MLX  (Apple)", "https://github.com/ml-explore/mlx",
     "Built into the search engine to run bge-m3 on the Mac's GPU, with the libraries its build pulls in: mlx-c, metal-cpp, "
     "fmt, nlohmann/json and gguflib.",
     ["mlx-LICENSE.txt", "mlx-c-LICENSE.txt", "metal-cpp-LICENSE.txt", "fmt-LICENSE.txt", "nlohmann-json-LICENSE.txt",
      "gguflib-LICENSE.txt"]),
    ("Oniguruma  (K. Kosako)", "https://github.com/kkos/oniguruma",
     "Built into the search engine (through the tokenizers crate) to split text into the model's tokens.",
     ["oniguruma-COPYING.txt"]),
    ("SQLite", "https://www.sqlite.org",
     "Built into the search engine (libsqlite3-sys) to store the search index. SQLite is in the public domain.",
     []),
    ("BAAI/bge-m3 model weights  (Beijing Academy of Artificial Intelligence)", "https://huggingface.co/BAAI/bge-m3",
     "Downloaded on first launch to find the passages that answer a question. Licensed under the MIT License.",
     []),
    ("kaldi-native-fbank  (Xiaomi Corporation and the Kaldi authors)", "https://github.com/csukuangfj/kaldi-native-fbank",
     "Ported to Rust and modified (engine/crates/tacie-embed/src/voice/fbank.rs) to compute the speaker models' input. "
     "Licensed under the Apache License, Version 2.0 (full text below, with the Rust crates).",
     []),
]

SWIFT = ["Sparkle", "posthog-ios"]


def run(cmd, cwd):
    return subprocess.run(cmd, cwd=cwd, check=True, capture_output=True, text=True).stdout


def header(title, url, use):
    return f"{RULE}\n{title}\n{url}\n{use}\n{RULE}\n"


def license_files(folder):
    names = ("license", "licence", "copying", "notice", "unlicense")
    files = [p for p in folder.iterdir() if p.is_file() and p.name.lower().startswith(names)]
    return sorted(files, key=lambda p: p.name.lower())


def crates():
    """(name, version, license, folder) of every crate in the shipped engine and MCP helper."""
    tree = run(["cargo", "tree", "-p", "tacie-engine", "-p", "tacie-mcp", "-e", "normal", "--prefix", "none",
                "--format", "{p}", "--target", "aarch64-apple-darwin"], ENGINE)
    wanted = set()
    for line in tree.splitlines():
        parts = line.replace(" (*)", "").replace(" (proc-macro)", "").split()
        if len(parts) >= 2 and not parts[0].startswith("tacie-"):
            wanted.add((parts[0], parts[1].lstrip("v")))
    meta = json.loads(run(["cargo", "metadata", "--format-version", "1", "--locked"], ENGINE))
    found = []
    for p in meta["packages"]:
        if (p["name"], p["version"]) in wanted:
            found.append((p["name"], p["version"], p.get("license") or "(see files)",
                          pathlib.Path(p["manifest_path"]).parent))
    missing = wanted - {(n, v) for n, v, _, _ in found}
    if missing:
        sys.exit(f"cargo metadata has no package for: {sorted(missing)}")
    return sorted(found)


def swift_checkouts():
    for base in (MAC / "build.noindex" / "SourcePackages" / "checkouts",
                 ROOT.parent / "tacie" / "apps" / "mac" / "build.noindex" / "SourcePackages" / "checkouts"):
        if all((base / s).is_dir() for s in SWIFT):
            return base
    sys.exit("no Swift package checkouts: build the app once (apps/mac/scripts/build.sh)")


def build():
    out = [(NOTICES / "components.txt").read_text().rstrip() + "\n\n"]
    for title, url, use, texts in COMPONENTS:
        out.append(header(title, url, use) + "\n")
        for t in texts:
            out.append((NOTICES / "texts" / t).read_text().rstrip() + "\n\n")
    base = swift_checkouts()
    for s in SWIFT:
        out.append(header(f"{s} (Swift package)", f"https://github.com/{'sparkle-project/Sparkle' if s == 'Sparkle' else 'PostHog/posthog-ios'}",
                          "Linked into the app.") + "\n")
        for f in license_files(base / s):
            out.append(f.read_text(errors="replace").rstrip() + "\n\n")
    # Rust crates: each distinct license text once, after the list of crates that carry it
    groups, bare = {}, []
    for name, version, lic, folder in crates():
        files = license_files(folder)
        if not files:
            bare.append(f"{name} {version}  ({lic})")
            continue
        text = "\n\n".join(f.read_text(errors="replace").strip() for f in files)
        groups.setdefault(text, []).append(f"{name} {version}  ({lic})")
    out.append(header("Rust crates in the search engine and the MCP helper", "https://crates.io",
                      f"{sum(len(v) for v in groups.values()) + len(bare)} crates, each listed with its license; "
                      "the license texts follow the crates that carry them.") + "\n")
    for text, names in sorted(groups.items(), key=lambda kv: kv[1][0]):
        out.append("\n".join(names) + "\n" + "-" * 80 + "\n" + text + "\n\n")
    if bare:
        out.append("These crates carry no license file of their own; their licenses are named here and their texts "
                   "are the standard ones above:\n" + "\n".join(bare) + "\n")
    return "".join(out).rstrip() + "\n"


def main():
    text = build()
    if "--check" in sys.argv:
        if not OUT.exists() or OUT.read_text() != text:
            sys.exit("ThirdPartyNotices.txt is out of date: run apps/mac/scripts/third-party-notices.py")
        print("ThirdPartyNotices.txt is up to date")
        return
    OUT.write_text(text)
    print(f"wrote {OUT.relative_to(ROOT)} ({len(text):,} bytes)")


if __name__ == "__main__":
    main()
