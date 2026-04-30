# Mistakes & Lessons Learned

> Record mistakes made during development and how to avoid them.
> This file is append-only - agents add discoveries, never delete.
>
> Format: Describe what went wrong, why, and how to avoid it next time.

## `vendor` recipe produced no tarball — consumer silently failed

**What happened:** The `justfile` `vendor` recipe ran `cargo vendor` and wrote `.cargo/config.toml`, but never executed `tar pcf vendor.tar vendor .cargo`. The `vendor-extract` recipe expected to unpack `vendor.tar`, so any offline build invoked via `just build-vendored` after `just vendor` failed with "file not found" — not a build error, just a missing artifact.

**Why:** The producer (vendor) and consumer (vendor-extract) were written without end-to-end verification. The recipe appeared complete because `cargo vendor` itself succeeded and printed no errors.

**Prevention:** When introducing a producer/consumer pair in a task runner (justfile, Makefile, etc.), run the consumer immediately after the producer in a clean directory to verify the contract. A recipe that creates an artifact for another recipe to consume must be tested as a pair, not in isolation.

**Fix:** Added `tar pcf vendor.tar vendor .cargo` as the final step of the `vendor` recipe so the tarball is produced before the intermediate directories are removed.
