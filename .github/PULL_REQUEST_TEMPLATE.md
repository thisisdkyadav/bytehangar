## Summary

<!-- What does this PR do and why? Link any related issues (e.g. "Closes #123"). -->

## Type of change

- [ ] Bug fix (non-breaking change that fixes an issue)
- [ ] New feature (non-breaking change that adds functionality)
- [ ] Breaking change (fix or feature that changes existing behavior/API)
- [ ] Documentation only
- [ ] Refactor / chore (no functional change)

## Testing done

<!-- Check what you ran; remove lines that don't apply. -->

- [ ] `cargo clippy --all-targets -- -D warnings` (in `server/`)
- [ ] `cargo test` (in `server/`)
- [ ] `npm run build` (in `sdk/`)
- [ ] `bash scripts/run-e2e.sh` (local backend)
- [ ] e2e with the S3/MinIO backend
- [ ] Manual testing — describe below

<!-- Notes on what you tested: -->

## Checklist

- [ ] My code follows the project's style and passes clippy with `-D warnings`
- [ ] I added/updated tests where appropriate
- [ ] I updated documentation (`README.md`, `sdk/README.md`, env tables) where needed
- [ ] Commits use clear, conventional-ish messages
- [ ] CI is green (clippy, tests, SDK build, e2e)
- [ ] I have not included any security-sensitive details that belong in a private report (see [SECURITY.md](../SECURITY.md))
