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
