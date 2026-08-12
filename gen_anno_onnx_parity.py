#!/usr/bin/env python3
"""Generate the anno-onnx release-parity reference from the real model.

Replicates the gliner SpanProcessor protocol (max_width=1) against the ONNX
export and writes evals/corpora/ner/anno_onnx_release_parity.json with the
reference entities per corpus text, for the Rust parity gate.
"""
import json
import numpy as np
import onnxruntime as ort
from tokenizers import Tokenizer

ROOT = "crates/memory-mcp/tests/models/ner/deepanwa--NuNerZero_onnx"
TOKEN_START, TOKEN_END, TOKEN_ENT, TOKEN_SEP = 1, 2, 128002, 128003
THRESHOLD = 0.5

tok = Tokenizer.from_file(f"{ROOT}/tokenizer.json")
cfg = json.load(open("/tmp/nuner_gliner_config.json"))
MAX_WIDTH = cfg["max_width"]
assert MAX_WIDTH == 1, f"parity assumes max_width=1, got {MAX_WIDTH}"

corpus = json.load(open("evals/corpora/ner/ner_quality.json"))
sess = ort.InferenceSession(f"{ROOT}/model.onnx", providers=["CPUExecutionProvider"])

def extract(text, labels):
    words = text.split()
    if not words or not labels:
        return []
    input_ids = [TOKEN_START]
    words_mask = [0]
    for label in labels:
        input_ids.append(TOKEN_ENT)
        words_mask.append(0)
        enc = tok.encode(label, add_special_tokens=False)
        input_ids.extend(enc.ids)
        words_mask.extend([0] * len(enc.ids))
    input_ids.append(TOKEN_SEP)
    words_mask.append(0)
    for i, word in enumerate(words):
        enc = tok.encode(word, add_special_tokens=False)
        input_ids.extend(enc.ids)
        words_mask.extend([i + 1] + [0] * (len(enc.ids) - 1))
    input_ids.append(TOKEN_END)
    words_mask.append(0)
    seq_len = len(input_ids)
    spans = [(i, i + j) for i in range(len(words)) for j in range(MAX_WIDTH)]
    feeds = {
        "input_ids": np.array([input_ids], dtype=np.int64),
        "attention_mask": np.ones((1, seq_len), dtype=np.int64),
        "words_mask": np.array([words_mask], dtype=np.int64),
        "text_lengths": np.array([[len(words)]], dtype=np.int64),
        "span_idx": np.array(spans, dtype=np.int64).reshape(1, -1, 2),
        "span_mask": np.ones((1, len(spans)), dtype=bool),
    }
    logits = np.asarray(sess.run(None, feeds)[0])
    probs = 1.0 / (1.0 + np.exp(-logits))[0]
    entities = []
    seen = set()
    for s, k, c in np.argwhere(probs > THRESHOLD):
        if s + k < len(words):
            name = " ".join(words[s : s + k + 1])
            if (name, labels[c]) not in seen:
                seen.add((name, labels[c]))
                entities.append({"name": name, "label": labels[c]})
    return entities

out = {"fixture_status": "release-parity-verified", "cases": []}
for case in corpus["cases"]:
    ents = extract(case["text"], case["labels"])
    out["cases"].append({"id": case["id"], "text": case["text"],
                         "labels": case["labels"], "entities": ents})
    print(f"{case['id']}: {len(ents)} entities -> {[(e['name'], e['label']) for e in ents]}")
with open("evals/corpora/ner/anno_onnx_release_parity.json", "w") as f:
    json.dump(out, f, indent=2, ensure_ascii=False)
print("wrote evals/corpora/ner/anno_onnx_release_parity.json")
