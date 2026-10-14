#!/usr/bin/env python3
# A local voice detection backend for live runs: LocalAI's POST {base}/vad
# (services/speech/src/voice.rs, `detect`), answered on Silero VAD v5, the
# model the original runs, so listening runs live without LocalAI.
#
# Usage: python3 scripts/vad-server.py PORT SILERO_VAD_ONNX
#   then FLOPPY_VOICE_BASE_URL=http://127.0.0.1:PORT/v1 in the env file.
#
# Needs numpy and onnxruntime. silero_vad.onnx ships in the silero-vad
# wheel (`pip download silero-vad==5.1.2 --no-deps`, then
# silero_vad/data/silero_vad.onnx). Spans follow silero-vad's
# get_speech_timestamps defaults (threshold 0.5, 250 ms minimum speech,
# 100 ms minimum silence, 30 ms padding).
import json, sys, http.server, socketserver
import numpy as np, onnxruntime as ort
sess = ort.InferenceSession(sys.argv[2], providers=["CPUExecutionProvider"])
SR, WIN, CTX = 16000, 512, 64

def probs(audio):
    state = np.zeros((2, 1, 128), np.float32)
    context = np.zeros((1, CTX), np.float32)
    out = []
    for i in range(0, len(audio), WIN):
        chunk = audio[i:i + WIN]
        if len(chunk) < WIN:
            chunk = np.pad(chunk, (0, WIN - len(chunk)))
        x = np.concatenate([context, chunk[None, :]], axis=1)
        p, state = sess.run(None, {"input": x, "state": state, "sr": np.array(SR, np.int64)})
        context = x[:, -CTX:]
        out.append(float(p[0][0]))
    return out

def segments(audio, threshold=0.5, min_speech_ms=250, min_silence_ms=100, pad_ms=30):
    ps = probs(audio)
    neg = threshold - 0.15
    min_speech, min_silence, pad = SR * min_speech_ms // 1000, SR * min_silence_ms // 1000, SR * pad_ms // 1000
    spans, start, temp_end = [], None, 0
    for i, p in enumerate(ps):
        at = i * WIN
        if p >= threshold:
            temp_end = 0
            if start is None:
                start = at
        elif p < neg and start is not None:
            if not temp_end:
                temp_end = at
            if at - temp_end >= min_silence:
                if temp_end - start > min_speech:
                    spans.append([start, temp_end])
                start, temp_end = None, 0
    if start is not None and len(audio) - start > min_speech:
        spans.append([start, len(audio)])
    for s in spans:
        s[0], s[1] = max(0, s[0] - pad), min(len(audio), s[1] + pad)
    return [{"start": round(a / SR, 3), "end": round(b / SR, 3)} for a, b in spans]

class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["content-length"])))
        if self.path.rstrip("/").endswith("/vad"):
            answer = {"segments": segments(np.asarray(body["audio"], np.float32))}
            code = 200
        else:
            answer, code = {"error": "not found"}, 404
        data = json.dumps(answer).encode()
        self.send_response(code); self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data))); self.end_headers(); self.wfile.write(data)
class S(socketserver.ThreadingMixIn, http.server.HTTPServer): daemon_threads = True
S(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
