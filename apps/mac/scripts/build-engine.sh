#!/bin/zsh
# Xcode build phase (project.yml postCompileScripts): build the Rust search engine and put it next to the app's
# executable (Contents/MacOS/tacie-engine), where EngineProcess finds it with Bundle.main.
# Runs on every build: cargo's own fingerprints make an unchanged engine a ~1 s no-op, and Xcode cannot list
# the crate sources as inputs without going stale. A missing cargo or a failed engine build fails the app build:
# an app without its engine would only fail once it runs.
# TACIE_CARGO picks the cargo binary (tests use it to simulate a Mac without Rust).
set -e
setopt pipefail
# CI's app tests need no engine (TacieTests has no app host): TACIE_SKIP_ENGINE=1 leaves it out, which saved the
# release engine build (about 16 minutes on GitHub's macOS runner). The app built this way cannot run meetings.
if [[ -n "${TACIE_SKIP_ENGINE:-}" ]]; then
  echo "engine build skipped (TACIE_SKIP_ENGINE)"
  exit 0
fi
# Xcode gives scripts a bare PATH; rustup installs into ~/.cargo/bin
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
cargo=${TACIE_CARGO:-$(command -v cargo || true)}
if [[ -z "$cargo" || ! -x "$cargo" ]]; then
  echo "error: cargo not found: install Rust with rustup to build the search engine" >&2
  exit 1
fi
dest="${TARGET_BUILD_DIR:?}/${EXECUTABLE_FOLDER_PATH:?}"
engine="${0:A:h}/../../../engine"
# Run cargo from the workspace (rustup reads engine/rust-toolchain.toml from the current folder) and without
# Xcode's environment: SDKROOT, MACOSX_DEPLOYMENT_TARGET and friends are part of cargo's fingerprints, so passing
# them through would make every terminal `cargo build --release` and every Xcode build rebuild the other's output.
# env -i also drops RUSTFLAGS, CARGO_TARGET_DIR (--target-dir pins the output to engine/target anyway) and ORT_*
# (onnxruntime download/link settings); to pass one of those through, add its name to `keep`.
# PATH is not passed through either: Xcode's script PATH puts the raw toolchain clang first, and without SDKROOT
# that clang cannot find the macOS SDK (C build scripts such as ring's fail on TargetConditionals.h). The
# /usr/bin/cc shim finds the SDK on its own, as in a terminal, so cargo gets cargo's folder plus the system folders.
keep=(HOME USER LOGNAME TMPDIR CARGO_HOME RUSTUP_HOME RUSTUP_TOOLCHAIN
      HTTP_PROXY HTTPS_PROXY ALL_PROXY NO_PROXY http_proxy https_proxy all_proxy no_proxy)
# The GPU embedder builds MLX from source (mlx-rs), which needs cmake and Xcode's Metal Toolchain. cmake usually
# lives in Homebrew's folder, outside the system folders, so its folder goes on cargo's PATH too.
cmake=$(command -v cmake || true)
if [[ -z "$cmake" ]]; then
  echo "error: cmake not found: install it (brew install cmake) to build the search engine's GPU embedder" >&2
  exit 1
fi
vars=("PATH=${cargo:h}:$HOME/.cargo/bin:${cmake:h}:/usr/bin:/bin:/usr/sbin:/sbin")
for k in $keep; do [[ -n "${(P)k}" ]] && vars+=("$k=${(P)k}"); done
# Every engine keeps the prompt text (#56, one app): the Tacie AI relay is called by prompt name and fills the text in
# itself, and the person's own AI (API key, on-device) is called with the text from here (prompts.rs by_name).
(cd "$engine" && env -i "${vars[@]}" "$cargo" build --release --manifest-path "$engine/Cargo.toml" \
  --target-dir "$engine/target")
mkdir -p "$dest"
release="$engine/target/release"
# The app runs on macOS 14 and on every Apple chip (M1 included). Two ways an engine build breaks that silently, so
# they fail the build here (engine/.cargo/config.toml sets MACOSX_DEPLOYMENT_TARGET and SOURCE_DATE_EPOCH for both):
# a binary that asks for a newer macOS, and whisper.cpp's ggml built for this Mac's own chip (-mcpu=native: i8mm's
# smmla on an M2+ crashes an M1 with SIGILL). The libggml check reads the newest whisper-rs-sys build; the
# onnxruntime library in the binary has smmla of its own, chosen at run time, so the binary as a whole is not counted.
if ! cmp -s "$release/tacie-engine" "$dest/tacie-engine" || ! cmp -s "$release/tacie-mcp" "$dest/tacie-mcp"; then
  for bin in tacie-engine tacie-mcp; do
    minos=$(otool -l "$release/$bin" | awk '/LC_BUILD_VERSION/{f=1} f&&/minos/{print $2; exit}')
    if [[ -z "$minos" ]] || ! [[ "$(printf '%s\n14.0\n' "$minos" | sort -V | tail -1)" == "14.0" ]]; then
      echo "error: $bin targets macOS ${minos:-unknown}; the app needs it to run on 14.0 (engine/.cargo/config.toml MACOSX_DEPLOYMENT_TARGET)" >&2
      exit 1
    fi
  done
  ggml=("$release"/build/whisper-rs-sys-*/out/lib/libggml-cpu.a(Nom[1]))
  if (( ${#ggml} )); then
    cache="${ggml[1]:h:h}/build/CMakeCache.txt"
    if grep -q '^GGML_NATIVE:BOOL=ON' "$cache" 2>/dev/null; then
      echo "error: whisper.cpp was built with GGML_NATIVE=ON ($cache); cargo clean -p whisper-rs-sys --release and build again" >&2
      exit 1
    fi
    smmla=0
    for a in "${ggml[1]:h}"/lib{ggml,whisper}*.a(N); do
      n=$(otool -tv "$a" | grep -c smmla || true)
      smmla=$(( smmla + n ))
    done
    if (( smmla > 0 )); then
      echo "error: whisper.cpp has $smmla i8mm (smmla) instructions that crash an M1 (${ggml[1]:h})" >&2
      exit 1
    fi
  fi
fi
# copy only what changed, so an unchanged engine keeps its timestamp and signature
copied=0
for f in "$release/tacie-engine" "$release/tacie-mcp" "$release"/libonnxruntime*.dylib(N); do
  if ! cmp -s "$f" "$dest/${f:t}"; then
    cp -f "$f" "$dest/${f:t}"
    copied=1
  fi
done
# The GPU kernels for sync embedding (engine/.cargo/config.toml): Contents/Resources/mlx.metallib, where the engine
# points MLX (tacie-embed gpu). Contents/MacOS takes only code: codesign refuses the app with it there.
res="${TARGET_BUILD_DIR}/${UNLOCALIZED_RESOURCES_FOLDER_PATH:?}"
metallib="$engine/target/mlx/mlx.metallib"
if [[ ! -f "$metallib" ]]; then
  echo "error: $metallib missing after the engine build (Xcode's Metal Toolchain: xcodebuild -downloadComponent MetalToolchain)" >&2
  exit 1
fi
mkdir -p "$res"
if ! cmp -s "$metallib" "$res/mlx.metallib"; then cp -f "$metallib" "$res/mlx.metallib"; copied=1; fi
rm -f "$dest/mlx.metallib" # an earlier build of this branch put it beside the engine
# The onboarding's sample indexes (docs/11 12절), when they were built: Contents/Resources/Sample (Korean, sample/index)
# and Contents/Resources/Sample-en (English, sample/en/index); the app picks one by its language (SampleLanguage).
# Missing is fine (the practice steps then need a connected repository; English falls back to the Korean one).
for from to in index Sample en/index Sample-en; do
  sample="${0:A:h}/../../../sample/$from"
  [[ -f "$sample/chunks.json" ]] || continue
  res="${TARGET_BUILD_DIR}/${UNLOCALIZED_RESOURCES_FOLDER_PATH:?}/$to"
  mkdir -p "$res"
  for f in "$sample"/*(N); do
    if ! cmp -s "$f" "$res/${f:t}"; then cp -f "$f" "$res/${f:t}"; copied=1; fi
  done
done
# This phase runs after Xcode decided whether CodeSign has work to do; a new engine inside an otherwise unchanged
# app would keep the old seal and fail `codesign --verify`. Touching the bundle makes CodeSign re-seal it.
if (( copied )); then
  touch -c "${TARGET_BUILD_DIR}/${WRAPPER_NAME:?}"
fi
