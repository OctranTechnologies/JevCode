# Foundation validation

Verified on 9 October 2026, Windows, Node 24.20.0 and Rust 1.95.0.

| Check | Result |
| --- | --- |
| `npm run tauri dev` | Desktop window launched and reported responding; Vite runs on port 1420 |
| `npm run lint` | Passed, zero lint warnings |
| `npm run typecheck` | Passed |
| `npm test` | 7 tests passed |
| `npm run build` | Passed |
| `cargo check --manifest-path src-tauri/Cargo.toml --all-targets` | Passed |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib` | 11 tests passed |
| `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` | Passed |
| `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` | Passed |
| `git diff --check` | Passed |

Frontend tests exercise real Tauri mock command/event boundaries, response and
event validation, listener cleanup, stale-snapshot ordering, shared wire fixtures,
and error redaction. Rust tests exercise real preview tool reads and persistence,
usage recording, permission pauses, deny enforcement, interrupted-run recovery,
path scoping, all remote wire protocols, text-only models and older Gemini calls
without wire IDs.

The browser UI was checked at 1280 × 850, 900 × 640 and 1539 × 1022. Provider and
usage navigation, suggestion-to-composer behavior, truthful browser disabled
states, and the minimum-height layout were inspected. The native executable was
launched; UI screenshots use browser mode and do not certify native end-to-end
credential entry or provider account access.

No remote API calls were made and no user credentials were added. Live provider
access, native keychain save/delete, signed release packages and cross-platform
launches remain unverified. Production browser assets build successfully; Rollup
reports harmless annotation warnings inside the installed Zod dependency.

The visual review's fix pass resolved three UI findings. Its remaining
`disposition: fix` is procedural: the corrected automated comparison reports
84.91% / match, but the hero gate still marks several visible controls as missing.
The unavailable external QUALITY BAR card and unclosed gate are recorded here;
no pixel-perfect reproduction claim is made. The remaining comparison evidence
does not affect lint, type checks, Rust checks or the runnable agent foundation.
