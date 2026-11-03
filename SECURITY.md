# Security policy

## Reporting a vulnerability

Please report security problems privately through GitHub's [private vulnerability reporting](https://github.com/vayungodara/cliproxy-rs/security/advisories/new), not in a public issue. Include the version (`cliproxy --version`), what an attacker can do, and the steps to reproduce. Leave real keys, tokens and account files out of the report.

You should get an answer within a week. Fixes go into a new release, and the advisory is published once users can update.

## Supported versions

Only the latest release gets security fixes.

## Verifying a release

The install scripts check each download against the `SHA256SUMS` file of the same GitHub release. That detects a corrupted or truncated download. It does not prove the release came from the maintainer, because anyone able to replace a release asset can replace both files. For a second source of trust, install through Homebrew, the AUR, winget or Scoop: their manifests pin the hashes in a separate repository.

## Usage output contains client keys

Each usage record (the usage queue, the v8 observability usage queue, the RESP `usage` channel and usage plugins) carries the full client API key in `api_key`, as CLIProxyAPI does, and `source` falls back to it. Treat a usage collector's output as secret: store it with the access you give `config.yaml`, and mask or drop the key before you ship records elsewhere.

## Scope

In scope: the cliproxy-rs server, its Management API and the built-in dashboard, for example a way around client-key or management-key checks, a credential or key leaking into logs or responses, or a request that reaches a provider with the wrong account.

Out of scope: a proxy deliberately exposed to the internet without `access.api-keys`, behaviour of the upstream providers, and problems in CLIProxyAPI itself (report those to [router-for-me/CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI)). [Running it safely](docs/GETTING-STARTED.md#running-it-safely) lists the settings a safe deployment needs.
