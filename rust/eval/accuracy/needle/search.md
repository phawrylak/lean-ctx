# Hybrid retrieval

Retrieval fuses a BM25 lexical arm with a dense vector arm using reciprocal rank
fusion, then reranks the merged candidate set before it enters the context window.
Spreading activation over the project graph pulls in files related to the top hits
even when they share no literal tokens with the query.
