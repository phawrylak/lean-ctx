# Provider prompt caching

Provider-side prompt caching rewards byte-stable prefixes. Anthropic bills cached
input tokens at a 90% discount versus fresh input, while OpenAI bills cached input
at roughly a 50% discount. lean-ctx keeps the carried conversation prefix
byte-identical across turns so the bulk of a long session is billed at the cheap
cached rate instead of the full input price.

This is why output determinism matters commercially: any timestamp or counter in a
tool-output body would change the bytes, break the cache key, and forfeit the
discount on every following turn.
