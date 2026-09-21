# Fluxcope MCP skill changelog

Skill versions are independent of application versions. Published releases use an immutable `skill-vVERSION` Git tag matching the skill's `metadata.version`. Withdrawn versions below retain their change records, but their releases and tag names are retired.

## 1.0.2 — 2026-09-21

- Replaces withdrawn releases 1.0.0 and 1.0.1 after correcting repository attribution. Their immutable tag names cannot be reused; install from `skill-v1.0.2`.
- Skill workflows, reference files, licenses, and the Fluxcope 0.2.0 interface baseline are unchanged.
- Release preparation now explicitly verifies the repository-local Git identity for commits, squash merges, and annotated tags.

## 1.0.1 — 2026-09-21 (withdrawn)

- Renamed the skill identity and directory from `fluxcope-mcp` to `fluxcope-skill`; MCP workflows and the Fluxcope 0.2.0 interface baseline are unchanged.
- Added repository shorthand, valid GitHub folder URL guidance, and migration instructions. Install the new name, then remove the old entry in the same scope; the installer does not rename existing installations.
- Originally released from `skills/fluxcope-skill` at the now-retired `skill-v1.0.1` tag.

## 1.0.0 — 2026-09-21 (withdrawn)

- Initial portable skill for Fluxcope's MCP interface, with native tools and managed CLI fallback.
- Guides for separate MCP registration, preview limitations, bounded capture inspection, recording, and revision-safe mapping changes.
- Targets the MCP interface in Fluxcope 0.2.0; older versions without MCP are unsupported. Check live capabilities when using another build.
- Originally released from `skills/fluxcope-mcp` at the now-retired `skill-v1.0.0` tag. License texts are included in the installed skill.

See the GitHub release notes for the exact source commit and completed validation.
