# Custom Embedding Models (`hf:org/repo`)

`ctx_semantic_search` ships three built-in ONNX models — but every team's corpus is
different. Since GL #397 you can point lean-ctx at **any HuggingFace repo with an ONNX
export**, fully local, no API keys:

```toml
# config.toml
[embedding]
model = "hf:intfloat/multilingual-e5-small@ffdcc22a9a5c973258343b0001a4d483cbd45be9"
```

Or per-shell:

```bash
export LEAN_CTX_EMBEDDING_MODEL="hf:intfloat/multilingual-e5-small@ffdcc22..."
```

The env var always wins over `config.toml`. The first semantic search after a model
switch triggers a **one-shot re-index** (the vector index stores the model id and
detects the mismatch automatically — no manual steps).

## What the repo must contain

| File | Purpose |
|---|---|
| `onnx/model.onnx` | The ONNX export of the encoder |
| `tokenizer.json` | HuggingFace fast-tokenizer config (WordPiece/BPE) |

Most `sentence-transformers`-compatible repos already publish both (look for the
`onnx/` folder on the model page). If yours doesn't, export it once:

```bash
pip install optimum[onnxruntime]
optimum-cli export onnx --model org/your-model --task feature-extraction ./out
# upload ./out/model.onnx as onnx/model.onnx + the generated tokenizer.json
```

## Syntax

```text
hf:<org>/<repo>[@<revision>]
```

- `revision` may be a tag, branch, or commit SHA. **Pin a revision.** Without a pin
  lean-ctx resolves `main` and logs a warning — an upstream force-push could otherwise
  change your embeddings silently.
- Optional `[embedding].dimensions` declares the vector width as a fallback. You can
  usually omit it: lean-ctx probes the real width from the ONNX graph at load time.

```toml
[embedding]
model = "hf:org/repo@v1.2"
dimensions = 1024   # optional fallback, probed value wins
```

## Supply-chain integrity

Downloads are cached under `~/.lean-ctx/models/hf-<org>-<repo>[-<rev>]/`. After the
first successful download lean-ctx writes a `model.lock.json` with the SHA-256 of every
artifact (trust-on-first-use). Any later re-download that doesn't reproduce the pinned
hash **fails hard** — a repo that swaps bytes under the same revision is rejected.

To intentionally accept new upstream content: delete the model directory (or just
`model.lock.json`) and re-run.

## Operational notes

- **Storage isolation**: every repo+revision combination gets its own directory, so
  switching back and forth never re-downloads.
- **Re-index semantics**: the index records the canonical model id
  (`hf:org/repo@rev`). Changing repo *or* revision re-indexes once; vectors from
  different models never mix.
- **Offline**: once downloaded, no network access is needed (or attempted).
- **Fallback**: if the model fails to load (bad export, unsupported tokenizer),
  semantic search degrades gracefully to BM25 — search keeps working.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `Unknown embedding model 'hf:…'` | Typo in repo id (must be exactly `org/repo`) | Check the repo URL on huggingface.co |
| `Download … returned HTTP 404` | Repo has no `onnx/model.onnx` at that revision | Export with `optimum-cli` (see above) or pick a repo with an ONNX export |
| `Failed to load tokenizer.json` | Repo ships no fast-tokenizer config or uses an unsupported model type | Re-export the tokenizer (`AutoTokenizer.from_pretrained(...).save_pretrained()` writes `tokenizer.json`) |
| `SHA-256 mismatch` | Upstream changed bytes under the same revision | Verify upstream intent, then delete `model.lock.json` to re-pin |
| Search results look wrong after switch | Old index still loading in a long-running daemon | `lean-ctx restart` — the re-index happens on the next search |

## Built-ins (no setup required)

| Alias | Model | Dims | Best for |
|---|---|---|---|
| `minilm` (default) | all-MiniLM-L6-v2 | 384 | Fast general-purpose |
| `jina-code-v2` | jina-embeddings-v2-base-code | 768 | Code + natural language |
| `nomic` | nomic-embed-text-v1.5 | 768 | Long-form text, MTEB-strong |
