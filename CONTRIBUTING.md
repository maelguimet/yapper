# Contributing

Thanks for helping improve Yapper.

## Setup

1. Clone the repository and read `README.md` (dev without install).
2. Use a local `.venv` with `pip install -e 'python[dev]'`.
3. Run checks before opening a PR:

```bash
cargo fmt --check
cargo test --locked
PYTHONPATH=python pytest -q -m 'not gpu'
./scripts/test_install_truth.sh
timeout 60s ./scripts/pip_audit.sh
```

GPU-marked tests are optional (`pytest -m gpu`).

## Scope

- Keep the product **local-only** (no cloud STT/TTS APIs in core paths).
- Do not commit model weights, proprietary voice WAVs, secrets, or machine-specific paths.
- Prefer small, reviewable commits with tests for behavior changes.

Public roadmap: use **GitHub Issues**; internal scratch boards (`TODO.md`, `GOAL.md`, `HANDOFF.md`) are gitignored and stay on maintainer machines only.

## API and agent clients

If you change `src/app/tts_api.rs` or `scripts/yapper-tts`, update `docs/tts-api.md` and `docs/agent-tts-tool.md` in the same change.

## License

By contributing, you agree your contributions are licensed under the MIT license in `LICENSE`.

## File-size cap

Each file stays under about 5,000 tokens, counted as 20,000 bytes. A pre-commit hook blocks bigger staged files, and CI runs the same check on every push and pull request. To enable the hook once per clone:

```sh
git config core.hooksPath .githooks
```

`scripts/check-file-size --all` checks the whole tree. When the check fails, split the file by responsibility, move tests to their own file, or extract data and fixtures. Some files were already over the cap when the check was added. They are listed in `.file-size-baseline` with their size at that time; they may shrink but not grow. Lockfiles, binary assets, and paths marked `linguist-generated` or `linguist-vendored` in `.gitattributes` are exempt.
