#!/usr/bin/env bash
# Download the fixed-URL model files into $1 (idempotent). Whisper weights come
# from the Hugging Face hub on first use (HF_HOME points inside VOICE_HOME).
set -euo pipefail
DIR="${1:?model dir}"
mkdir -p "$DIR"
fetch() {
  local name="$1" url="$2"
  if [ ! -s "$DIR/$name" ]; then
    echo "fetching $name" >&2
    curl -sSL --retry 3 -o "$DIR/$name.part" "$url" && mv "$DIR/$name.part" "$DIR/$name"
  fi
}
fetch silero_vad.onnx  https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx
fetch kokoro-v1.0.onnx https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/kokoro-v1.0.onnx
fetch voices-v1.0.bin  https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/voices-v1.0.bin
