"""Best-effort model -> context-window presets.

Only a fallback: Hermes passes the real ``context_length`` to
``update_model``; these published windows are used when it does not. Matching
is by case-insensitive substring, longest key first (so ``gpt-4o-mini`` is not
shadowed by ``gpt-4``).
"""

from __future__ import annotations

from typing import Optional

# Conservative, widely-documented context windows.
_PRESETS = {
    "claude": 200_000,
    "claude-3": 200_000,
    "claude-4": 200_000,
    "claude-opus-4-8": 1_000_000,
    "claude-opus-4-7": 1_000_000,
    "claude-opus-4-6": 1_000_000,
    "claude-opus-4-5": 200_000,
    "claude-sonnet-4-6": 1_000_000,
    "claude-sonnet-4-5": 1_000_000,
    "claude-haiku": 200_000,
    "claude-fable": 1_000_000,
    "claude-mythos": 1_000_000,
    "gpt-4o": 128_000,
    "gpt-4.1": 1_000_000,
    "gpt-4-turbo": 128_000,
    "gpt-4": 8_192,
    "gpt-3.5": 16_385,
    "o1": 200_000,
    "o3": 200_000,
    "hermes": 128_000,
    "llama-3.1": 128_000,
    "llama-3": 8_192,
    "qwen2.5": 128_000,
    "deepseek": 128_000,
    "mistral": 32_768,
    "gemini-1.5": 1_000_000,
    "gemini": 1_000_000,
}


def context_length_for(model: Optional[str]) -> Optional[int]:
    """Return a known context window for ``model``, or ``None`` if unknown."""
    if not model:
        return None
    needle = model.lower()
    # Claude ids are hyphenated; a dotted variant ("claude-opus-4.8") is non-canonical
    # but appears in some traffic. Also match a dot->hyphen normalized form, scoped to
    # claude so GPT/Gemini keys that legitimately use dots (gpt-4.1) are left alone.
    needles = [needle]
    if needle.startswith("claude") and "." in needle:
        needles.append(needle.replace(".", "-"))
    # Order-independent match for full claude ids: the family/version order flipped
    # between generations (version-first claude-3-5-sonnet vs family-first
    # claude-opus-4-8), so accept either ordering (claude-4-8-opus == claude-opus-4-8).
    # Runs before the substring pass so a reversed id isn't shadowed by claude-3/claude-4.
    if needle.startswith("claude"):
        want = sorted(t for t in needle.replace(".", "-").split("-") if t)
        for key, val in _PRESETS.items():
            if key.startswith("claude") and key.count("-") >= 2 and sorted(key.split("-")) == want:
                return val
    for key in sorted(_PRESETS, key=len, reverse=True):
        if any(key in n for n in needles):
            return _PRESETS[key]
    return None
