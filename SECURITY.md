# Security Policy

ByteHangar is a security-positioned storage product: it accepts **untrusted uploads** from end-user clients, mints signed grants, and serves files back. We take vulnerabilities in it seriously and appreciate responsible disclosure.

## Supported versions

Security fixes are provided for the latest `1.1.x` release line.

| Version | Supported |
|---------|-----------|
| 1.1.x   | ✅ |
| < 1.1   | ❌ |

Always run the most recent `1.1.x` patch release.

## Reporting a vulnerability

**Please do not open a public GitHub issue, pull request, or discussion for security problems.**

Report privately through either channel:

1. **GitHub Security Advisories** (preferred) — open a private report at
   <https://github.com/thisisdkyadav/bytehangar/security/advisories/new>.
   This keeps the details confidential and lets us collaborate on a fix and a coordinated disclosure.
2. **Email** — <thedkyadav@outlook.com>. Use a subject line beginning with `[ByteHangar Security]`.

When you report, please include as much of the following as you can:

- A description of the vulnerability and its impact.
- The affected version (or commit) and the configured byte backend (`local` or `s3`).
- Step-by-step reproduction, ideally with a minimal proof of concept.
- Any relevant configuration (e.g. CORS allow-list, `TRUST_FORWARDED_FOR`, `ALLOW_PRIVATE_OUTBOUND`).

## Scope

ByteHangar handles untrusted input by design. Reports that are especially in scope include, but are not limited to:

- Bypassing **grant** signing, single-use nonce enforcement, or the policy ∩ grant ∩ global-cap bounds.
- Bypassing the global content-type allowlist, `MAX_UPLOAD_BYTES`, or path-safe category enforcement.
- Cross-tenant access to files, metadata, signing secrets, or webhook secrets.
- Forging signed download URLs, or accessing private files without a valid signed URL / download-auth approval.
- SSRF via tenant-controlled URLs (webhook + download-auth callbacks).
- Disclosure of secrets at rest (tenant signing/webhook secrets, `MASTER_KEY`-encrypted data).
- Authentication/authorization flaws on the internal or public plane.

Out of scope: issues that require an already-compromised host or database, missing hardening that is explicitly the operator's responsibility (TLS termination, L7/DDoS protection, and keeping the internal plane private), and findings against unsupported versions.

## Response expectations

- **Acknowledgement** of your report within **3 business days**.
- An initial **assessment and severity triage** within **7 business days**.
- For confirmed issues, we aim to ship a fix in a `1.1.x` patch release and publish a coordinated advisory crediting you (unless you prefer to remain anonymous).

Thank you for helping keep ByteHangar and its users safe.
