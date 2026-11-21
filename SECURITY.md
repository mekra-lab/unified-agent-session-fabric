# Security policy

## Reporting a vulnerability

Report suspected vulnerabilities privately through GitHub's private vulnerability
reporting: open the repository's **Security** tab and choose **Report a vulnerability**.
Please keep vulnerability details out of public issues, discussions and pull requests.

Include the affected component (a crate under `crates/` or `tools/handoff-receipts`),
the commit or version, steps to reproduce and the impact you observed. Use synthetic
data in reports: leave out real session contents, credentials, account identifiers
and private file paths.

## Scope and expectations

All components are experimental and not yet production-ready. There are no supported
release branches and no response-time commitment; reports are handled on a best-effort
basis. Known limits, such as the synthetic-only index and page probes and the page
probe's ability to expose bodies blocked by core policy, are documented in
[Security and limits](docs/security-and-limits.md) and need no separate report.
