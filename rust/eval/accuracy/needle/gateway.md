# Gateway prose roles

The lean-ctx gateway can compress natural-language system and user turns, but only
inside the frozen region of a request, so the provider prompt cache is never
invalidated. Per-role aggressiveness lets an operator squeeze the system prompt
harder than user turns, because system preambles are usually more repetitive.
