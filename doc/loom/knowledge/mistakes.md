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

## Static visualization range silently hides out-of-band data

**What happened:** `src/sparkline.rs` originally hardcoded the y-axis to `[10.0, 40.0]` °C with the rationale "anything outside the band visually pins at the edge." In practice, values outside the band did NOT pin at the edge — the y-mapping `pad + (1 - norm) * usable_h` produced coordinates outside the 16 px canvas, and the polyline silently disappeared. Users observed "the sparkline doesn't always appear" for cold-boot reads (< 10 °C) or thermal events (> 40 °C). The same code also early-returned an empty frame when `samples.len() < 2`, so the sparkline was blank for ~1.5 s after the first reading.

**Why:** Two design assumptions, both wrong:
1. "Pin at the edge" was assumed without verifying the coordinate math — the comment described intent, not behavior. There was no clamp or visibility check at the canvas boundary.
2. The `< 2` threshold optimized for the polyline math (`(n - 1)` denominator) without considering UX — the user-visible cost of a blank widget was higher than the implementation cost of a single-sample fallback.

**Prevention:**
- For any visualization with a fixed numeric range, render at least one out-of-range test case manually (or as a unit test) and confirm the result is visible. A comment claiming "pins at the edge" is not verification.
- For canvas/draw code with sample-count branches, list every n in `{0, 1, 2, many}` and decide what should render for each, not just "the common case."
- Extract the math helper (e.g. `y_range`) as a pure function so the scaling behavior is unit-testable without an iced renderer. Six tests on the new `y_range` would have caught the original silent-clipping bug at design time.

**Fix:** Replaced fixed range with `y_range(&[f64]) -> (f64, f64)` auto-scaling helper with a `MIN_Y_SPAN = 2.0` °C floor (centered on midpoint) to prevent noise amplification on flat traces. Single-sample case now renders a horizontal tick across the canvas at the sample's y. Six unit tests added for `y_range`.

## Claiming "zero pedantic warnings" without running the full command (2026-06-12)

**What happened:** A code review asserted "zero `-W clippy::pedantic` warnings" based on `just check` output. `just check` runs `cargo clippy --all-features -- -W clippy::pedantic` (no `--all-targets`). The full command `cargo clippy --all-targets --all-features -- -W clippy::pedantic` surfaces ~14 pre-existing pedantic warnings (e.g. `doc_markdown` on type names in doc comments, `too_many_lines` on `update`, `cast_*` on bounded cast helpers with justifying in-code comments). The review claim was inaccurate.

**Why:** The `just check` invocation omits `--all-targets`, which excludes test targets from linting. Pedantic warnings in test code (and in code only reachable through tests) are invisible without `--all-targets`.

**Prevention:**
- Always run the EXACT command being asserted, including all flags, before claiming a lint gate is clean.
- `just check` ≠ `just ci-local` ≠ the full pedantic command. Know which gate you're checking.
- Background subagents cannot answer permission prompts interactively, so lint verification commands must run in a foreground/main-agent context where prompts can be accepted.

**Detection heuristic:** if a lint claim says "clean" but `just check` was the only evidence, re-run `cargo clippy --all-targets --all-features -- -W clippy::pedantic` and count warnings before asserting.

## Manual version bump without regenerating Cargo.lock broke CI `--locked` (2026-06-12)

**What happened:** `v0.3.1` was cut by hand-editing `version` in `Cargo.toml` (commit `chore: update version for new release`) without regenerating `Cargo.lock`, so the lock's own `liquidmon` package entry still said `0.3.0`. CI runs `cargo build --release --locked`; cargo needs to rewrite the lock to reconcile the two versions, but `--locked` forbids that, so the release build failed with "cannot update the lock file ... because --locked was passed." The local working tree happened to have a regenerated lock, but it was never committed, masking the problem locally.

**Why:** The `liquidmon` package's version appears in BOTH `Cargo.toml` and `Cargo.lock`. A manual `Cargo.toml`-only bump leaves them inconsistent. The inconsistency is invisible to any non-`--locked` cargo command (which silently fixes the lock in place), so it only surfaces in CI.

**Prevention:**
- Bump releases with `just tag <X.Y.Z>`, not by hand. That recipe edits every `Cargo.toml`, then runs `cargo check` to regenerate `Cargo.lock`, stages the lock, commits `release: <ver>`, and tags. (Note: the recipe does NOT touch `resources/app.metainfo.xml` — add the `<release>` entry separately before committing.)
- If bumping manually, run `cargo check --locked` afterward as a pre-commit gate — it exits non-zero on exactly the mismatch CI would hit, before you commit.
- Stage `Cargo.lock` in the same commit as the `Cargo.toml` version change; never let a version bump and its lock update land in separate commits.

**Fix:** Committed the regenerated `Cargo.lock` to sync `0.3.1`, then cut `0.3.2` with `Cargo.toml` + `Cargo.lock` + metainfo in one `release:` commit, verified by `cargo check --locked`, and deleted the stale never-pushed `v0.3.1` tag.
