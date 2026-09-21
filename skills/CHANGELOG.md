# Fluxcope MCP skill changelog

Skill versions are independent of application versions. Each entry corresponds to an immutable `skill-vVERSION` Git tag and the skill's `metadata.version`.

## 1.0.1 — 2026-09-21

- Renamed the skill identity and directory from `fluxcope-mcp` to `fluxcope-skill`; MCP workflows and the Fluxcope 0.2.0 interface baseline are unchanged.
- Added repository shorthand, valid GitHub folder URL guidance, and migration instructions. Install the new name, then remove the old entry in the same scope; the installer does not rename existing installations.
- Installation uses `skills/fluxcope-skill` at `skill-v1.0.1`. The original `skill-v1.0.0` tag and `skills/fluxcope-mcp` path are preserved.

## 1.0.0 — 2026-09-21

- Initial portable skill for Fluxcope's MCP interface, with native tools and managed CLI fallback.
- Guides for separate MCP registration, preview limitations, bounded capture inspection, recording, and revision-safe mapping changes.
- Targets the MCP interface in Fluxcope 0.2.0; older versions without MCP are unsupported. Check live capabilities when using another build.
- Installation uses the `skills/fluxcope-mcp` directory at `skill-v1.0.0`. License texts are included in the installed skill.

See the GitHub release notes for the exact source commit and completed validation.
