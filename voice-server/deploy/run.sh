#!/usr/bin/env bash
# Install (first run) and start the voice server, keeping everything under one directory.
#
#   VOICE_HOME=/opt/arbos-voice VOICE_TOKEN=... deploy/run.sh [server args]
#
# Layout under $VOICE_HOME (default: the parent of this script's directory):
#   .venv/     Python env (uv)        models/    VAD, Kokoro weights      hf/  Whisper weights
#   bin/       uv, cloudflared        logs/      server + tunnel logs
# Nothing is written outside $VOICE_HOME except ~/.ssh by whoever logs in.
set -euo pipefail

SRC="$(cd "$(dirname "$0")/.." && pwd)"
VOICE_HOME="${VOICE_HOME:-$SRC}"
export HF_HOME="$VOICE_HOME/hf" UV_CACHE_DIR="$VOICE_HOME/.uv-cache" UV_PYTHON_INSTALL_DIR="$VOICE_HOME/.uv-python"
export XDG_CACHE_HOME="$VOICE_HOME/.cache"
mkdir -p "$VOICE_HOME/bin" "$VOICE_HOME/models" "$VOICE_HOME/logs" "$HF_HOME"
export PATH="$VOICE_HOME/bin:$PATH"

if ! command -v uv >/dev/null; then
  curl -LsSf https://astral.sh/uv/install.sh | UV_INSTALL_DIR="$VOICE_HOME/bin" UV_NO_MODIFY_PATH=1 sh
fi

if [ ! -x "$VOICE_HOME/.venv/bin/python" ]; then
  uv venv -q --python 3.12 "$VOICE_HOME/.venv"
fi
# shellcheck disable=SC1091
source "$VOICE_HOME/.venv/bin/activate"

if command -v nvidia-smi >/dev/null 2>&1 && nvidia-smi -L >/dev/null 2>&1; then
  uv pip install -q -e "$SRC[gpu]"
  # onnxruntime and onnxruntime-gpu ship the same module; keep only the CUDA build.
  if ! python -c "import onnxruntime as o; assert 'CUDAExecutionProvider' in o.get_available_providers()" 2>/dev/null; then
    uv pip uninstall -q onnxruntime onnxruntime-gpu 2>/dev/null || true
    uv pip install -q "onnxruntime-gpu>=1.20"
  fi
  export ONNX_PROVIDER=CUDAExecutionProvider
  # CTranslate2 (faster-whisper) dlopens cuBLAS/cuDNN from the pip wheels.
  export LD_LIBRARY_PATH="$(python -c 'import os, nvidia.cublas.lib, nvidia.cudnn.lib; print(os.path.dirname(nvidia.cublas.lib.__file__)+":"+os.path.dirname(nvidia.cudnn.lib.__file__))'):${LD_LIBRARY_PATH:-}"
else
  uv pip install -q -e "$SRC"
fi

"$SRC/deploy/fetch-models.sh" "$VOICE_HOME/models"

exec python -m voice_server --model-dir "$VOICE_HOME/models" "$@"
