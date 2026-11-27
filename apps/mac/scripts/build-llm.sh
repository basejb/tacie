#!/bin/zsh
# Builds llama.cpp's server at a pinned release into Contents/MacOS/tacie-llm: on-device meeting notes run on it, with
# no Ollama to install (issue #54, docs/41 0절; every build since #56). The app starts it only in on-device mode with the
# notes model downloaded (BundledLLM); it listens on 127.0.0.1 and unloads the model when idle.
# The source comes from the release's tarball (hash-checked) and builds once per release under build.noindex/llama.cpp;
# later builds only copy the binary. TACIE_SKIP_ENGINE=1 (CI's app tests) leaves it out, as it does the engine.
set -e
setopt pipefail
if [[ -n "${TACIE_SKIP_ENGINE:-}" ]]; then
  echo "llama.cpp server build skipped (TACIE_SKIP_ENGINE)"
  exit 0
fi
tag=b12261
sha=6c1e1e9054495da89f8d957663ba955978947047318edcf4195de0ce1ac70bb9
dest="${TARGET_BUILD_DIR:?}/${EXECUTABLE_FOLDER_PATH:?}"
root="${0:A:h}/../build.noindex/llama.cpp"
src="$root/llama.cpp-$tag"
out="$root/build-$tag"
bin="$out/bin/llama-server"
cmake=$(PATH="/opt/homebrew/bin:/usr/local/bin:$PATH" command -v cmake || true)
if [[ -z "$cmake" ]]; then
  echo "error: cmake not found: install it (brew install cmake) to build the on-device notes server" >&2
  exit 1
fi

if [[ ! -x "$bin" ]]; then
  mkdir -p "$root"
  if [[ ! -d "$src" ]]; then
    tarball="$root/$tag.tar.gz"
    curl -sL --fail -o "$tarball" "https://github.com/ggml-org/llama.cpp/archive/refs/tags/$tag.tar.gz"
    if [[ "$(shasum -a 256 "$tarball" | cut -d' ' -f1)" != "$sha" ]]; then
      rm -f "$tarball"
      echo "error: llama.cpp $tag tarball does not match its hash" >&2
      exit 1
    fi
    tar xzf "$tarball" -C "$root"
    rm -f "$tarball"
  fi
  # Without Xcode's environment, as build-engine.sh does for cargo: Xcode's PATH puts a raw clang first that cannot
  # find the SDK. macOS 14 and every Apple chip: GGML_NATIVE off (a native build on an M2+ crashes an M1). One static
  # binary with the Metal kernels inside; no HTTPS, no web page download, no subprocesses (single-model server).
  env -i HOME="$HOME" PATH="${cmake:h}:/usr/bin:/bin:/usr/sbin:/sbin" "$cmake" -S "$src" -B "$out" -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_OSX_DEPLOYMENT_TARGET=14.0 -DCMAKE_OSX_ARCHITECTURES=arm64 -DBUILD_SHARED_LIBS=OFF \
    -DGGML_NATIVE=OFF -DGGML_METAL=ON -DGGML_METAL_EMBED_LIBRARY=ON -DGGML_BLAS=OFF \
    -DLLAMA_OPENSSL=OFF -DLLAMA_SUBPROCESS=OFF -DLLAMA_USE_PREBUILT_UI=OFF -DLLAMA_BUILD_UI=OFF \
    -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF -DLLAMA_BUILD_SERVER=ON -DLLAMA_BUILD_APP=OFF > "$root/cmake-$tag.log"
  env -i HOME="$HOME" PATH="${cmake:h}:/usr/bin:/bin:/usr/sbin:/sbin" "$cmake" --build "$out" --target llama-server \
    -j "$(sysctl -n hw.ncpu)" >> "$root/cmake-$tag.log"
  [[ -x "$bin" ]] || { echo "error: llama-server not built (see $root/cmake-$tag.log)" >&2; exit 1; }
  n=$(otool -tv "$bin" | grep -c smmla || true)
  if (( n > 0 )); then
    rm -f "$bin"
    echo "error: llama-server has $n i8mm (smmla) instructions that crash an M1" >&2
    exit 1
  fi
fi
mkdir -p "$dest"
cmp -s "$bin" "$dest/tacie-llm" || cp -f "$bin" "$dest/tacie-llm"
