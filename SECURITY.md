# Security

Fluxcope is a debugging MITM proxy. Use it only with traffic and devices you are authorized to inspect. It listens on all IPv4 interfaces with no proxy authentication. Restrict access with your host firewall or use a trusted private network.

Captured URLs, headers, bodies and logs can contain credentials and personal data. Do not include real captures or `~/.fluxcope` in public bug reports. Configuration, CA material and logs are stored with owner-only permissions. Only the public CA PEM is exposed by the temporary download server; never share the CA private key.

Trust the public certificate only in test clients that need HTTPS inspection. Remove that trust and disable the client proxy when finished. Never bypass TLS verification to make a request work. See [README.md](README.md) for CA location and cleanup guidance.

Report vulnerabilities through the repository's **Security → Report a vulnerability** private reporting flow once enabled. Do not post exploit details or secrets in a public issue. If private reporting is unavailable, wait for the maintainer to provide a private channel before sharing sensitive details.

The first release supports macOS 15 or newer and the Linux/glibc environments documented in the README. macOS binaries are unsigned and not notarized; verify their checksum and GitHub provenance before execution.

Dependency advisories run in CI. No known vulnerability is currently allowlisted. One transitive build-time macro (`paste`) has an unmaintained advisory with a documented, expiring exception in `.github/advisory-exceptions.json`.

Known upstream operational limitation: Hudsucker 0.25 immediately retries listener errors. If the process exhausts its file descriptors, repeated failed accepts may consume CPU until descriptors become available. Avoid exposing the unauthenticated proxy to untrusted clients; this release does not claim denial-of-service resilience. A maintained upstream fix should replace this version when available.
