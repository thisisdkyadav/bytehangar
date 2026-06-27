---
name: Bug report
about: Report a problem with ByteHangar
title: "[bug]: "
labels: bug
assignees: ""
---

## Description

A clear and concise description of what the bug is.

## Steps to reproduce

1. ...
2. ...
3. ...

## Expected behavior

What you expected to happen.

## Actual behavior

What actually happened. Include error messages, logs (with `x-request-id` if available), and stack traces where relevant.

## Environment

- ByteHangar version / commit:
- Byte backend: <!-- local | s3 (S3/MinIO/R2/B2) -->
- How it's run: <!-- cargo run | docker | other -->
- OS / arch:
- SDK version (if relevant):

## Relevant configuration

Any non-default env vars (e.g. `STORAGE_BACKEND`, `ALLOWED_ORIGINS`, `TRUST_FORWARDED_FOR`, `ALLOW_PRIVATE_OUTBOUND`). **Do not paste secrets** (`ADMIN_TOKEN`, `MASTER_KEY`, S3 credentials).

## Additional context

Add any other context, screenshots, or minimal reproduction here.

> Found a **security** vulnerability? Do not file it here — follow [SECURITY.md](../../SECURITY.md) for private disclosure.
