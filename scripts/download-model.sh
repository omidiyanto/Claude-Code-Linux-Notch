#!/usr/bin/env bash
# Downloads a model for Claude Notch and checks it against a pinned SHA-256.
# Usage: scripts/download-model.sh [small-q5_1|small|large-v3-turbo-q5_0|base|kokoro]
#   kokoro: the voice that reads answers aloud (Kokoro-82M, ~330 MB), with two voices.
set -euo pipefail

dir="${XDG_DATA_HOME:-$HOME/.local/share}/claude-notch/models"

# fetch URL FILE SHA256: downloads unless FILE is already there and verified.
fetch() {
  local url="$1" file="$2" sha="$3"
  mkdir -p "$(dirname "$file")"
  if [[ -f "$file" ]] && echo "$sha  $file" | sha256sum --check --status; then
    echo "$file is already there and verified."
    return
  fi
  echo "Downloading $url"
  curl --fail --location --proto '=https' --tlsv1.2 --progress-bar --output "$file.part" "$url"
  if ! echo "$sha  $file.part" | sha256sum --check --status; then
    rm -f "$file.part"
    echo "Checksum mismatch: the download was corrupted or tampered with. Nothing was installed." >&2
    exit 1
  fi
  mv "$file.part" "$file"
  echo "Installed $file"
}

model="${1:-small-q5_1}"
case "$model" in
  base) sha=60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe ;;
  small-q5_1) sha=ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb ;;
  small) sha=1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b ;;
  large-v3-turbo-q5_0) sha=394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2 ;;
  kokoro)
    # A fixed revision, so the pinned sums stay valid.
    base="https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/1939ad2a8e416c0acfeecc08a694d14ef25f2231"
    fetch "$base/onnx/model.onnx" "$dir/kokoro/model.onnx" 8fbea51ea711f2af382e88c833d9e288c6dc82ce5e98421ea61c058ce21a34cb
    fetch "$base/tokenizer.json" "$dir/kokoro/tokenizer.json" 77a02c8e164413299b4b4c403b14f8e0e1c1b727db4d46a09d6327b861060a34
    fetch "$base/voices/af_heart.bin" "$dir/kokoro/voices/af_heart.bin" d583ccff3cdca2f7fae535cb998ac07e9fcb90f09737b9a41fa2734ec44a8f0b
    fetch "$base/voices/am_michael.bin" "$dir/kokoro/voices/am_michael.bin" 1d1f21dd8da39c30705cd4c75d039d265e9bc4a2a93ed09bc9e1b1225eb95ba1
    echo "Turn on \"Read answers aloud\" in the settings, or set in ~/.config/claude-notch/config.toml:  speak = true"
    echo "Voices: af_heart (default), am_michael:  voice = \"am_michael\""
    exit 0
    ;;
  *)
    echo "unknown model '$model' (choose: small-q5_1, small, large-v3-turbo-q5_0, base, kokoro)" >&2
    exit 2
    ;;
esac

fetch "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-$model.bin" "$dir/ggml-$model.bin" "$sha"
if [[ "$model" != "small-q5_1" ]]; then
  echo "Set it in ~/.config/claude-notch/config.toml:  model_path = \"$dir/ggml-$model.bin\""
fi
