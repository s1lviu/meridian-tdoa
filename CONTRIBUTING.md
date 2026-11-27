# Contributing

Use English for documentation, interface text, issues and pull requests. Keep changes focused and describe what changed and how it was verified.

Run the relevant checks from the README. Changes to capture, GNSS parsing, DSP or localization should include a meaningful regression check. UI changes should be checked in Chromium at desktop and mobile sizes. The CI workflow uses the bundled real recording and a fixed public directory snapshot; it does not open public SDR capture sessions.

Preserve the measurement contract:

- Position is derived exclusively from TDoA. Known transmitter coordinates belong only in the separate evaluator.
- A receiver directory, an open socket or a configured frequency is not evidence of RF reception or identity.
- Preserve original IQ and hashes. Label replay, synthetic tests and injected faults explicitly.
- Do not turn an ambiguous solution into a unique result using the known beacon position.
- Source adapters need primary protocol documentation, access-policy evidence and tests of synchronization failures.
- Never commit administrator tokens, provider credentials, runtime captures or local environment files.

By contributing, you license your contribution under GPL-3.0-or-later. Keep attribution and upstream license notices when adding third-party assets. Do not add a new data source without documenting its provenance and redistribution terms.
