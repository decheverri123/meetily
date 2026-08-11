# Cutting token/context cost in Meetily

Research report — 2026-08-11. 107 agents, 25 sources, 119 claims extracted, 11 survived adversarial verification.

Read this top to bottom if you're new to RAG. Section 2 is the explainer; sections 3–5 are the actual recommendations.

---

## 0. The headline: this is a recall problem wearing a cost problem's clothes

Before the research, I checked what your code actually does today. Three constants in
[frontend/src-tauri/src/summary/commands.rs](../frontend/src-tauri/src/summary/commands.rs):

| Constant | Line | Value | ≈ tokens |
|---|---|---|---|
| `ASK_MEETING_CONTEXT_MAX_CHARS` | [476](../frontend/src-tauri/src/summary/commands.rs#L476) | 40,000 chars | ~10,000 |
| `ASK_LIVE_TRANSCRIPT_CONTEXT_MAX_CHARS` | [492](../frontend/src-tauri/src/summary/commands.rs#L492) | 40,000 chars | ~10,000 |
| `ASK_ACROSS_MEETINGS_CONTEXT_MAX_CHARS` | [484](../frontend/src-tauri/src/summary/commands.rs#L484) | 100,000 chars | ~25,000 |

Your token spend is **already bounded**. You cannot blow up a bill; the ceiling is hard. So "save tokens"
is not actually the pressing problem.

What those ceilings cost you instead is **answer quality, silently**:

- `take_last_chars` ([554](../frontend/src-tauri/src/summary/commands.rs#L554)) keeps the **last** 40,000 characters.
  A one-hour meeting transcribes to roughly 48,000 characters. So on any meeting longer than ~50 minutes,
  **the beginning is thrown away** — and the beginning is where agendas, goals, and framing live.
  "What did we decide we were trying to achieve?" is unanswerable on a long meeting, and the model
  doesn't know it's missing, so it answers confidently from the tail.
- `build_cross_meeting_context` ([647](../frontend/src-tauri/src/summary/commands.rs#L647)) packs meeting
  **summaries** (not transcripts) until it hits 100,000 chars, then **drops whole meetings** and appends an
  "omitted N" note. At ~1,500 chars per summary that's roughly 65 meetings before truncation begins.
  Past that, "when did we first discuss X?" silently cannot see the meeting where you first discussed X —
  and it's chronologically ordered, so it's *always the same meetings* that fall off.

**Retrieval fixes both.** Instead of "the most recent 10k tokens", you send "the 10k tokens most relevant
to this question", drawn from the entire corpus. Same budget, dramatically better recall — and usually a
smaller budget too, which is where the cost saving shows up as a side effect.

That reframing matters for how you prioritize: the cross-meeting path is the one that's actually broken,
and it's also the one that genuinely needs RAG. Do it first.

---

## 1. Three paths, three different fixes

"Just add RAG" would be the wrong call. Your three ask surfaces have genuinely different shapes:

| Path | Code | Corpus size | Right fix |
|---|---|---|---|
| Live in-progress meeting | `ask_about_live_transcript` | Grows, unbounded | **Rolling summary + recent window.** Not vector search. |
| One saved meeting | `ask_about_meeting` | ~12k tokens, bounded | **Prompt caching + hierarchical chunks.** RAG is optional. |
| Entire meeting database | `ask_across_meetings` | Millions of tokens | **Real RAG.** Nothing else works. |

The rest of the report walks each one.

---

## 2. How this actually works (the explainer)

You said you don't know much about this. Here's the mechanical version — no hand-waving.

### 2.1 Embeddings: text as coordinates

An **embedding model** is a neural net that eats a chunk of text and outputs a fixed-length list of
numbers — a **vector**. For example, `bge-small-en-v1.5` outputs 384 numbers. `nomic-embed-text` outputs 768.

The useful property: the model is trained so that text with **similar meaning** lands at nearby coordinates.
"We should ship on Friday" and "let's release at end of week" produce vectors that point in nearly the same
direction, even though they share almost no words. That's the whole trick — it's semantic matching, not
keyword matching.

"Nearby" is measured by **cosine similarity**: the cosine of the angle between two vectors. 1.0 = identical
direction, 0 = unrelated, −1 = opposite. Computing it is one dot product and two magnitudes — trivial arithmetic.

So **search** becomes: embed the user's question into a vector, compute cosine similarity against every stored
chunk vector, sort, take the top K. That's it. That's the entire core of vector search.

**Dimensionality is the cost lever.** 384 numbers per chunk is half the storage and half the arithmetic of 768.
Verified benchmark below shows this is the single biggest performance decision you'll make.

### 2.2 Chunking: what you embed

You don't embed a whole meeting — one vector can't represent an hour of conversation with any precision.
You split it into **chunks** (typically a few hundred tokens each) and embed each one. Retrieval returns chunks.

Chunk size is a tradeoff:
- **Too small** — a chunk says "yeah, exactly" with no referent. Retrieved, it's useless.
- **Too big** — the chunk is mostly irrelevant filler, the vector is a mush of averaged meanings, and it
  matches everything weakly and nothing strongly.

**Overlap** (each chunk repeats the last ~50 tokens of the previous one) prevents a decision that straddles
a boundary from being cut in half.

⚠️ **The research found no verified evidence on chunk sizing for meeting transcripts specifically.** Every claim
in this area was refuted or unverified. Treat any number you read (including mine) as a starting guess to
measure, not a finding.

For your app the shape is somewhat forced anyway: you already have **speaker-turn segments with timestamps**,
and your `askCitations` lib resolves `[MM:SS]` citations back to segments. So chunk on **speaker-turn boundaries,
grouped to roughly 200–400 tokens, with one turn of overlap**, and carry `segment_id` + start/end timestamp as
metadata on every chunk. That keeps your existing citation-chip feature working through retrieval — the chunk
knows which segments it came from, so the answer can still cite `[MM:SS]` and highlight the right transcript rows.

### 2.3 Indexes: brute force vs. ANN

To find the top K, you *could* compare against every stored vector — a **brute-force** (exhaustive) scan.
It's O(n), it's exact, and it's boring in the best way: 100% recall, no tuning, no failure modes.

An **ANN index** (Approximate Nearest Neighbor — HNSW, IVF) builds a graph or clustering so you only compare
against a small candidate subset. Sublinear, much faster at scale, but **approximate** — it can miss results,
and every index has knobs that trade recall for speed.

**The question is only: at what size does brute force stop being fast enough?** Here's the verified answer
(section 4.2). Spoiler: not at your scale.

### 2.4 Hybrid search: vectors miss exact strings

Embeddings are bad at exact tokens. Search for an error code, a person's name, a ticket ID, or a product
codename and dense vector search will happily return semantically-adjacent chunks that don't contain it.

**BM25** is the classic keyword ranking algorithm (a refined TF-IDF: rare words weigh more, long documents
get penalized). SQLite ships it as **FTS5**, already built in — you don't add a dependency.

**Hybrid** runs both and fuses the ranked lists. **Reciprocal Rank Fusion (RRF)** is the standard fusion:
each document scores `Σ 1/(k + rank)` across the lists (k≈60). It uses only ranks, never raw scores, so you
don't need to normalize a cosine similarity against a BM25 score — which is the thing that makes naive
score-blending fragile.

### 2.5 Reranking: a second, slower, better pass

Your embedding model is a **bi-encoder**: it embeds question and chunk *separately*, then compares. Fast
(chunks are embedded once, offline) but the model never sees the pair together.

A **cross-encoder reranker** takes `(question, chunk)` as a single joined input and scores relevance directly.
Far more accurate, far slower — so you never run it over the corpus. You use it as stage two: retrieve ~50
candidates cheaply, rerank those 50, keep the top 5.

This is the biggest measured quality win in the whole research (section 4.4).

---

## 3. Path A + B: the single-meeting and live paths (no RAG needed)

### 3.1 Prompt caching — the lever you're not using

*(Facts verified against current Anthropic API docs, not the web research.)*

When you send the same prompt prefix repeatedly, the API can cache the processed prefix. Mechanics:

- **Prefix match.** The cache key is the exact bytes up to a `cache_control` breakpoint. **One byte
  different anywhere in the prefix invalidates everything after it.**
- **Render order** is `tools` → `system` → `messages`. Stable content first, volatile content last.
- **Pricing:** cache reads ≈ **0.1× input price**. Cache writes cost **1.25×** (5-minute TTL) or **2×** (1-hour TTL).
- **Break-even:** 2 requests at 5m TTL, 3 requests at 1h TTL.
- **Minimum cacheable prefix is model-dependent** and *not* monotonic across generations:

  | Model | Minimum |
  |---|---:|
  | Claude Opus 5 | 512 tokens |
  | Claude Sonnet 5, Opus 4.8 | 1,024 tokens |
  | Haiku 4.5 | 4,096 tokens |

  Below the minimum it silently doesn't cache — no error, just `cache_creation_input_tokens: 0`.
- **Verify** with `usage.cache_read_input_tokens`. Zero across repeated requests = a silent invalidator.
- Max 4 breakpoints per request.

**The math for your single-meeting ask**, 40k chars ≈ 10k tokens, Claude Opus 5 at $5/MTok input:

| | Cost per question |
|---|---|
| Today (uncached) | 10,000 × $5/1M = **$0.050** |
| First question (cache write, 1.25×) | **$0.063** |
| Every question after (cache read, 0.1×) | 10,000 × $0.50/1M = **$0.005** |

**10× cheaper from the second question onward**, zero retrieval infrastructure, no embedding model, no index.
Note this only pays off for *follow-up* questions on the same meeting — a single one-shot question costs
slightly more. Your ask panels are conversational, so follow-ups are the common case.

### 3.2 ⚠️ Your current truncation actively destroys caching

This is the one thing to fix first, and it's cheap.

`take_last_chars` keeps the **tail**. During a live meeting the transcript grows, so the 40k-char window
**slides forward** — every new question has a *different* prefix. Even if you added `cache_control` today,
your hit rate would be approximately zero.

The fix is to make the context **append-only**: keep a stable head (a rolling summary of everything before the
window) plus a growing tail, with the breakpoint placed at the end of the stable head. Then each new question
extends the prefix rather than shifting it, and the cache hits.

Same issue applies to any per-request volatile content — a timestamp, a UUID, a `chrono::Utc::now()` in the
system prompt sits at the front of the prefix and invalidates everything downstream. Audit for those.

### 3.3 Live meetings: rolling summary, not vector search

For an in-progress meeting, the right structure is:

```
[stable: rolling summary of minutes 0..N-5]   ← cache breakpoint here
[volatile: verbatim transcript, last 5 minutes]
[the question]
```

Every ~5 minutes, fold the oldest verbatim window into the rolling summary with a cheap model (Haiku 4.5 at
$1/MTok, or local Ollama — this is a summarization task, not a reasoning task). The stable head grows slowly
and append-only, so it caches. The volatile tail stays small.

This also fixes the recall bug: minute 3 of a two-hour meeting is *in the summary* rather than discarded.

⚠️ **Caveat the research could not resolve:** rolling/hierarchical summarization produced **no verified claims**.
The structure above is standard practice and follows from the caching mechanics, but it isn't backed by a
benchmark in this research. Measure it.

⚠️ **Whisper revision hazard:** if your pipeline ever *revises* an already-emitted segment (partial transcripts
getting corrected), that rewrites the middle of the prefix and blows the cache. Check whether
`transcript-update` events are strictly append-only. If they aren't, only fold segments into the stable head
once they're finalized.

---

## 4. Path C: cross-meeting — the verified RAG stack

This is the path that's actually broken and actually needs retrieval. Here's what survived verification.

### 4.1 Storage: `sqlite-vec`, in your existing DB file

**Confidence: high (3–0).** [sqlite-vec](https://github.com/asg017/sqlite-vec) is a SQLite extension for vector
storage and search. The Rust crate is genuinely first-party (`crates.io` repository field → `asg017/sqlite-vec`;
84 versions, 2.3M downloads).

The build script is:

```rust
cc::Build::new().file("sqlite-vec.c").define("SQLITE_CORE", None).compile("sqlite_vec0");
```

`SQLITE_CORE` is SQLite's compile-into-core macro — so vector search is **statically linked into your Tauri
binary**. No `.dylib`/`.dll` sidecar to bundle, sign, or notarize. For a desktop app that ships to macOS and
Windows, this is the deciding factor.

⚠️ **Three real risks:**

1. **Pre-1.0 alpha** (`0.1.10-alpha.4`). README: *"sqlite-vec is a pre-v1, so expect breaking changes!"*
   Commit `6e2c4c6` changed both the SQL API **and the on-disk vtab schema** *inside the alpha line*, gating on
   an `_info` shadow-table version. For an app carrying persistent user SQLite files, that's exactly the wrong
   kind of churn. **Version your vector tables and write a migration path on day one.**
2. **You use `sqlx`, not `rusqlite`** ([Cargo.toml:147](../frontend/src-tauri/Cargo.toml#L147) —
   `sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "chrono"] }`). The documented extension
   registration is a `rusqlite` auto-extension hook. **The sqlx path is unverified** — validate that
   `sqlite3_auto_extension` registration works through sqlx's connection setup before committing to this.
   This is the single biggest integration unknown; spike it first.
3. There was a ~6-month maintenance gap (issue #226) before 2026 releases resumed, and the crates.io crate can
   lag the extension.

**`sqlite-vss` is dead** (confidence: high, 3–0). Author's README: *"sqlite-vss is not in active development.
Instead, my effort is now going towards sqlite-vec."* No code commits since 2024-05-05 (~27 months). It also
depends on Faiss, which has **no Windows support** — disqualifying for Tauri regardless.

No verified evidence was gathered on LanceDB, usearch, hnsw_rs, or embedded Qdrant. They're options, not
benchmarked recommendations.

### 4.2 Brute force is fine at your scale — dimensionality is what matters

**Confidence: high (3–0).** Measured on an M1 mini / 8GB, 100k vectors stored **on disk**, average KNN latency:

| Dimensions | Latency @ 100k vectors |
|---:|---:|
| 192 / 384 / 768 / 1024 | **< 75 ms** |
| 1536 | 105 ms |
| 3072 | 214 ms |

Corroborated by an *adversarial* source — vectorlite's own benchmarks, published to argue HNSW beats brute
force, measured sqlite-vec at 3.86 ms for 3k×1536d and 7.78 ms for 20k×512d, extrapolating to the same order
of magnitude. The author also publishes his failure case (1M vectors: 8.52 s at 3072d), which is the opposite
of cherry-picking. Test hardware is two Apple Silicon generations old, so these numbers are pessimistic for
current machines.

**200 meetings ≈ 20k–40k chunks. You are far inside the comfortable zone.** No ANN index. No recall tuning.
Exact search, which eliminates an entire category of "why didn't it find that" bugs.

⚠️ **Pick a 384–768 dimension model.** Dimensionality is the dominant latency lever — 1024d and below stays
under 75 ms, 3072d is 3× worse. A **Matryoshka** model (trained so you can truncate the vector and keep most
of the quality) lets you drop dimensions later without re-embedding everything.

⚠️ **Two things the benchmark did NOT measure:**
- **Metadata pre-filtering** (filter by date/meeting, *then* vector search) was unmeasured and was a roadmap
  item. This underpins the "search my last quarter" experience — verify it yourself.
- Crisp breakpoint claims ("100k is where brute force becomes impractical", "1M is the ceiling") were both
  **refuted 0–3**. Quote the measured latencies, not a breakpoint.

Binary-quantized (`bit`) vectors were dramatically faster — 11 ms even at 3072d — if you can tolerate recall
loss. Worth knowing as an escape hatch.

### 4.3 Embeddings: `fastembed-rs`, in-process, offline

**Confidence: high (3–0).** [fastembed-rs](https://github.com/Anush008/fastembed-rs) (v5.17.4, published
2026-07-28; ~1.5M recent downloads — actively maintained) runs embedding models locally via ONNX Runtime.

Verified against the actual generated API docs:

- Models download once and load from cache afterward — **no network at runtime**. `UserDefinedEmbeddingModel`
  loads local ONNX weights with no download at all, which is the right story for privacy-first bundling.
- It **also ships cross-encoder rerankers** (next section) — one dependency covers both stages.
- ⚠️ **It provides no persistent index.** The entire public API is two modules; `similarity` has exactly three
  free functions (`cosine_similarity`, `dot`, `top_k`) and `top_k` is a linear scan by construction. README:
  *"For larger corpora or persistence, push the vectors to a vector search engine."* That's what sqlite-vec is for.
  Don't expect fastembed to store anything.

⚠️ **One claim scored only 1–2 and needs re-checking before you code against it**: that fastembed is pure-Rust,
`ort`-backed, has **no Tokio dependency** (so it's directly callable from a Tauri command handler), defaults to
`bge-small-en-v1.5` at 384 dims, and defaults to batch size 256. Verify on docs.rs first — the Tokio question
matters for how you wire it into your async command layer.

**On `nomic-embed-text`** (confidence: medium): 137M params, Apache-2.0, 8192-token context, runnable via
Ollama or ONNX. Three operational traps:

1. **The 8192 context is not free.** llama.cpp/Ollama GGUF metadata reports `context_length` 2048 and needs
   explicit `num_ctx` / RoPE extension. **A naive Ollama embedding call silently truncates long chunks**
   (ollama issue #11214).
2. The `search_document:` / `search_query:` **task prefixes are mandatory** for correct query/document asymmetry.
   Omit them and quality drops with no error.
3. Candle is the weak path — `candle-transformers` covers BERT/JinaBERT/T5, not nomic-bert. Rust access is
   really via fastembed.

⚠️ **The claim that nomic-embed-text-v1 matches OpenAI `text-embedding-3-small` on MTEB (62.39 vs 62.26) was
REFUTED 0–3.** So "local costs you nothing in quality" is **not established**. If you offer the hybrid
local/cloud switch you wanted, you should measure the gap on your own transcripts rather than assume parity.
Also: v1 is a Feb-2024 model, already superseded by v1.5 (Matryoshka), v2-moe, EmbeddingGemma, and Qwen3-Embedding.
**The specific model recommendation is the fastest-aging thing in this report.**

### 4.4 Reranking is the biggest quality win available

**Confidence: medium (3–0, heavily qualified).** From *"From BM25 to Corrective RAG"*
([arXiv 2604.01733v1](https://arxiv.org/html/2604.01733v1), Apr 2026), 23,088 queries over 7,318 documents:

Adding a Cohere cross-encoder reranker on top of hybrid BM25+dense RRF:

| Metric | Hybrid RRF | + Reranker | Change |
|---|---:|---:|---|
| Recall@5 | 0.695 | **0.816** | +17.4% rel. |
| MRR@3 | 0.433 | **0.605** | +39.7% rel. |

The paper's own abstract calls it *"the largest improvement in the study."*

⚠️ **Six qualifiers, all material to you:**

1. Financial documents only; the authors' limitations section says findings *"may not generalize."*
2. **All answers are numerical** — near-best-case for a cross-encoder, far from transcript QA.
3. **Retrieval was whole-document** (avg 920 tokens), not passage-chunked. The authors explicitly warn
   *"performance patterns may differ for chunked corpora"* — so the headline number is **untested on the
   chunk shape you'd actually use.**
4. Queries were semi-synthetic, reformulated by Llama-3.3-70B to inject entities (7.3% → 83.9%
   context-independent) — which favors lexical+rerank stacks.
5. Cohere Rerank v4.0 Pro was itself benchmarked on finance at release (possible train/eval alignment).
6. **No open or local reranker was evaluated.** This is zero direct evidence for `bge-reranker` running locally.

Unreviewed preprint. A companion claim about reranking depth (top-20 → R@5 0.458 vs top-50 → 0.826) was
**refuted 0–3**, so don't hard-code an over-fetch depth of 50 on that basis — tune it.

Still: it's the largest lever found, fastembed gives you `bge-reranker-base`/`v2-m3` locally with no extra
dependency, and the downside is bounded latency. Worth building, worth measuring.

### 4.5 ⚠️ The thing that will actually bite you: follow-up questions

**Confidence: high (3–0). This is the most directly applicable finding in the report.**

From [MTRAG](https://direct.mit.edu/tacl/article/doi/10.1162/TACL.a.19/132114/mtRAG-A-Multi-Turn-Conversational-Benchmark-for)
(TACL 2025, IBM Research, **peer-reviewed** — the strongest source here):

| Turn | Recall@5 |
|---|---:|
| Turn 1 (opening question) | **0.89** |
| All later turns | **0.47** |

Retrieval quality **halves after the first question.** Verification killed the obvious confound: unanswerable
questions are excluded from the metric, so 0.47 is genuine failure on questions that *do* have gold evidence.
Arithmetic cross-check confirms attribution: (102×0.89 + 675×0.47)/777 = 0.525, matching the reported 0.52.

**Why this matters specifically for you:** you built `useAskAI` with conversation-thread support — `AskTurn`
history, threaded Q&A in `LiveAskPanel`. Your UI is *designed* for follow-ups. So this is your dominant
failure mode, not an edge case. "What did they decide?" after "Tell me about the API redesign" embeds as a
near-meaningless query with no referent.

**The mitigation: contextual query rewriting.** Before retrieval, run a cheap LLM pass that rewrites the
follow-up into a standalone question using the conversation history. "What did they decide?" →
"What did the team decide about the API redesign?" — *then* embed that.

Improvement was **consistent across all 24 measured cells** (3 retrievers × 8 metrics), zero exceptions:

| Retriever | Recall@10 before | after |
|---|---:|---:|
| BM25 | 0.27 | 0.33 |
| BGE-base-1.5 (dense) | 0.38 | 0.47 |
| Elser (sparse) | 0.58 | 0.64 |

The baseline isn't a strawman — the authors tested full-conversation and subset-of-conversation query
construction and found both consistently *worse* than last-turn-only, so rewriting is measured against the
strongest alternative.

⚠️ **Five qualifiers:**
1. Corpora are **written documents** (Wikipedia, finance, govt, cloud docs), **not transcripts**. Spoken
   transcripts are plausibly *harder* — denser pronouns, weaker self-containment.
2. The collapse is driven mainly by turn **position** (standalone later turns still sit at 0.48), not purely
   anaphora — so "resolve the pronouns and you're fixed" overstates it. Rewriting is partial, not a cure.
3. Rewriter was only Mixtral 8x7B, so the gains are a **floor**.
4. Separate literature (arXiv 2509.22325) finds **keyword-list/HyDE-style rewrites HURT** while
   *decontextualizing* rewrites help. MTRAG used the decontextualizing style — **use that style specifically.**
5. One appendix slice breaks the consistency (Cloud domain R@5 0.48 → 0.47), and no significance tests were reported.

⚠️ **Refuted, do not implement:** the claim that stuffing history into the retrieval query is *actively harmful*
and you should embed only the rewritten last turn scored **0–3**. The rewriting benefit survived; the
"never include history" prescription did not. Test both.

---

## 5. Recommended build order

Sequenced by value-per-unit-effort, not by architectural tidiness.

### Phase 0 — free wins, no new dependencies

1. **Make the single-meeting context append-only and add `cache_control`.** ~10× cost reduction on follow-up
   questions, and it's a handful of lines. Verify with `usage.cache_read_input_tokens` — if it's zero, you
   have a silent invalidator.
2. **Audit the prompt prefix for volatile content** — timestamps, UUIDs, anything per-request sitting ahead
   of the transcript.
3. **Add contextual query rewriting to all three ask paths.** Cheap (one short LLM call, Haiku or local
   Ollama), and it's the verified fix for your dominant failure mode. This helps *today*, before any
   retrieval exists, because it also improves the truncated-context answers.
4. **Add FTS5 keyword search over transcript segments.** SQLite already has it; zero new dependencies. On its
   own it fixes the "find the meeting where we mentioned $SPECIFIC_THING" case that's currently broken by
   truncation.

### Phase 1 — the cross-meeting fix

5. **Spike the sqlx + sqlite-vec integration first.** This is the biggest unknown and it's binary — if
   extension registration doesn't work through sqlx, the whole storage plan changes. Do not build on top of
   it until this is proven.
6. **Chunk on speaker-turn boundaries** (~200–400 tokens, one turn overlap), carrying `segment_id` and
   start/end timestamps as metadata so your existing `[MM:SS]` citation chips keep working.
7. **Embed with fastembed-rs**, 384–768 dims, Matryoshka model. Index in background on transcript save, not
   on the UI thread. Store a schema version and model identifier alongside the vectors from day one — you
   *will* change embedding models, and re-embedding is a migration.
8. **Retrieve hybrid: FTS5 + vector, fused with RRF.** Replace `build_cross_meeting_context`'s
   "pack summaries until full, drop the rest" with "retrieve the most relevant chunks across all meetings."

### Phase 2 — quality

9. **Add local cross-encoder reranking** via fastembed's `TextRerank`. Over-fetch candidates, rerank, keep
   top ~5. Measure the added latency on Apple Silicon — the source paper reported only aggregate throughput,
   never per-query latency, so you're flying blind on this until you measure it.
10. **Hierarchical cross-meeting retrieval**: search meeting-level summaries first to shortlist meetings, then
    drill into chunks of the winners. Reduces the search space and preserves cross-meeting narrative.
    ⚠️ No verified evidence — this is a design intuition, not a finding.

### Phase 3 — live path

11. **Rolling summary for live meetings**, folding finalized segments into a stable cached head every ~5 min.
    Check whether your `transcript-update` pipeline is strictly append-only first (section 3.3).

### Upgrade path (when personal scale isn't enough)

Brute force degrades linearly. The escalation ladder, cheapest first:
1. **Metadata pre-filtering** by date/meeting — scan a subset (verify it's supported and fast).
2. **Binary quantization** — 11 ms at 3072d in the same benchmark.
3. **Dimension truncation** via Matryoshka — free if you picked the right model.
4. **Only then** an ANN index (usearch, hnsw_rs, or an external engine). Trigger is roughly the low hundreds
   of thousands of chunks — sooner with high dimensionality. None of these were benchmarked in this research.

---

## 6. Measurement

⚠️ **The research produced no verified claims on measurement methodology.** This section is standard practice,
not a finding.

**Tokens:** log `usage.input_tokens`, `cache_read_input_tokens`, and `cache_creation_input_tokens` per ask.
Don't estimate — use `messages.count_tokens` for pre-flight sizing. Never `tiktoken`; it's OpenAI's tokenizer
and undercounts Claude by 15–20% on prose, more on code.

**Retrieval quality:** hand-label ~50 questions against your own meetings with the segment IDs that *should*
be retrieved. Measure recall@k (did the right chunk make the top k?) and MRR (how high did it rank?).
Fifty labeled examples is enough to catch a regression; you do not need a research-grade eval set.

**Answer quality:** the cheapest useful test is A/B — same question, full-context answer vs retrieved-context
answer, blind-judged. If retrieval loses, your chunking or retrieval depth is wrong, not the concept.

**Measure follow-ups separately from opening questions.** Given the 0.89 → 0.47 finding, an eval set of only
opening questions will look great and tell you nothing about how the app actually behaves.

---

## 7. What this research did NOT establish

Being straight about coverage: 119 claims were extracted, 25 verified, **11 survived**. Five of eight
sub-questions produced **nothing verified**:

- **Transcript chunking** — no evidence at all on fixed-token vs semantic vs speaker-turn vs sliding-window,
  on chunk size/overlap for meetings, or on carrying timestamp+speaker metadata through retrieval.
- **Non-retrieval token reduction** — prompt caching economics, map-reduce/hierarchical summarization,
  rolling summaries, LLMLingua compression, history trimming. (I filled the prompt-caching gap separately
  from current Anthropic API docs; the rest remains open.)
- **Cross-meeting architecture** — metadata pre-filtering and hierarchical summary-then-drill-down.
- **Indexing operations** — when to embed, incremental re-index on edit, dimension-change migrations,
  on-disk index size.
- **Measurement** — recall@k/MRR/nDCG practice, RAGAS faithfulness, cheap A/B methodology.

**Refuted claims — do not recycle these, they sound plausible and failed verification:**

| Claim | Vote |
|---|---|
| Anthropic contextual embeddings: 35% retrieval-failure reduction | 0–3 |
| Contextual embeddings + BM25 + rerank: 47% failure reduction | 0–3 |
| nomic-embed-text-v1 matches OpenAI text-embedding-3-small on MTEB | 0–3 |
| Stuffing history into the retrieval query is actively harmful | 0–3 |
| "~100k vectors is where brute force becomes impractical" | 0–3 |
| "1M vectors is the practical ceiling" | 0–3 |
| Reranker depth: top-20 R@5 0.458 vs top-50 0.826 | 0–3 |
| USearch vs LanceDB recall/speed comparison | 0–3 |
| Anthropic prompt-caching contextualization savings (61.83% / ~69%) | 1–2 |
| BM25 beats dense retrieval on every metric | 1–2 |
| fastembed runtime details (Tokio-free, default model, batch size) | 1–2 |

That last one matters: **re-verify the Tokio question on docs.rs before you code against it.**

**The biggest uncertainty is domain transfer.** Zero retrieval-quality numbers here were measured on meeting
transcripts. MTRAG's corpora are written documents; the reranker paper is financial text-and-table with
all-numerical answers and unchunked retrieval. Spoken transcripts differ in exactly the ways that matter:
pronoun density, disfluency, weak self-containment, no headings for a reranker to key on.

**Re-measure on your own data before trusting any number in this report.**

**Source quality, honestly:** two peer-reviewed TACL-grade sources (MTRAG), one unreviewed preprint
(reranking), and the rest are library READMEs, generated API docs, registry metadata, and one maintainer
self-benchmark. The library facts are as solid as such facts get — verified against build scripts and enum
definitions, not marketing copy. The performance and quality figures are weaker.

---

## 8. Open questions worth answering before you commit

1. **Does the sqlx path work with sqlite-vec at all?** Highest-value spike. Binary outcome, changes everything downstream.
2. **Do local rerankers reproduce the cloud reranker's gains?** No open reranker was evaluated. This decides
   whether privacy-first costs you real quality. Also: what's per-query CPU latency for a local cross-encoder
   over ~50 candidates on Apple Silicon? The paper reported only aggregate throughput.
3. **Does sqlite-vec support metadata pre-filtering + KNN at usable speed?** The 100k benchmark was
   *unfiltered*; filtering was a roadmap item. Both your cross-meeting architecture and the hierarchical
   design assume it works.
4. **Is your `transcript-update` pipeline strictly append-only?** Determines whether live-meeting prompt
   caching is viable at all.
5. **How much quality do local embeddings actually cost vs cloud, on transcripts?** The parity claim was
   refuted. This is the deciding input for the hybrid local/cloud switch you asked about.
