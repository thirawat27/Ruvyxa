# Security Policy

## Supported Versions

| Version | Supported |
|---|---|
| 1.x | Yes |

## Reporting a Vulnerability

Report vulnerabilities privately through GitHub security advisories for:

https://github.com/thirawat27/ruvyxa

Do not open a public issue for suspected vulnerabilities.

Please include:

- affected package and version
- operating system and Node version
- reproduction steps
- expected impact
- whether a workaround exists

## Security Baseline

Ruvyxa 1.0 includes:

- server/client boundary validation
- private environment variable blocking in client bundles
- same-origin and Fetch Metadata checks for server actions
- action body limits and content-type checks
- in-process action rate limiting
- default security headers for runtime responses
- deterministic BLAKE3 client asset hashes
- native CLI packages for supported OS/CPU targets

Apps should still add deployment-layer controls such as TLS, CDN/WAF rules, secret rotation, and a custom Content Security Policy when required.
