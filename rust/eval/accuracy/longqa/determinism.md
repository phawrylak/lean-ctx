# Output determinism

A lean-ctx tool output is a deterministic function of file content, read mode, CRP
mode and the active task. Two runs on the same inputs produce byte-identical text,
which is what makes the result safe to cache and cheap to re-bill.

To hold that contract the runtime keeps no timestamps or counters in output
bodies; artifact filenames are content-addressed via a BLAKE3 hash of the command
that produced them. Dedicated regression tests fail the build if any tool starts
leaking a non-deterministic byte into its body.
