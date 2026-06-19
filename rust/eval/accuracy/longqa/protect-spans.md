# Protect spans

Verbatim regions can be fenced with lc_safe markers or named with a protect
parameter so the compressor preserves them exactly. License headers, cryptographic
hashes, base64 blobs and generated code stay byte-for-byte intact while the prose
around them is still squeezed.

Protection is reported back to the caller: the response notes how many spans were
held verbatim, so a reviewer can see that nothing inside a protected region was
rewritten or dropped during compression.
