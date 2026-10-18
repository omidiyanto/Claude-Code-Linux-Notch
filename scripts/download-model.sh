#!/usr/bin/env bash
# Downloads a Whisper model for Claude Notch and checks it against a pinned SHA-256.
# Usage: scripts/download-model.sh [small-q5_1|small|large-v3-turbo-q5_0|base]
set -euo pipefail

model="${1:-small-q5_1}"
case "$model" in
  base) sha=60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe ;;
  small-q5_1) sha=ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb ;;
  small) sha=1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b ;;
  large-v3-turbo-q5_0) sha=394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2 ;;
  *)
    echo "unknown model '$model' (choose: small-q5_1, small, large-v3-turbo-q5_0, base)" >&2
    exit 2
    ;;
esac

dir="${XDG_DATA_HOME:-$HOME/.local/share}/claude-notch/models"
file="$dir/ggml-$model.bin"
mkdir -p "$dir"

if [[ -f "$file" ]] && echo "$sha  $file" | sha256sum --check --status; then
  echo "$file is already there and verified."
  exit 0
fi

url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-$model.bin"
echo "Downloading $url"
curl --fail --location --proto '=https' --tlsv1.2 --progress-bar --output "$file.part" "$url"
if ! echo "$sha  $file.part" | sha256sum --check --status; then
  rm -f "$file.part"
  echo "Checksum mismatch: the download was corrupted or tampered with. Nothing was installed." >&2
  exit 1
fi
mv "$file.part" "$file"
echo "Installed $file"
if [[ "$model" != "small-q5_1" ]]; then
  echo "Set it in ~/.config/claude-notch/config.toml:  model_path = \"$file\""
fi
