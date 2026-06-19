# Aggressiveness knob

lean-ctx exposes a single compression-intensity control called the aggressiveness
knob. It accepts one continuous value from 0.0 to 1.0: at 0.0 a read is returned
losslessly, and at 1.0 the compressor applies its maximum density target. Every
read mode maps onto this one number, so callers never have to juggle per-mode
flags to dial accuracy against token savings.

The effective value resolves from the request parameter first, then the session
default, then the configured proxy default. Inputs outside the band are clamped
back into the 0.0 to 1.0 range before the density target is derived.
