# Releasing the Fluxcope MCP skill

The skill is distributed from GitHub through the skills CLI. Its source is `skills/fluxcope-skill/`; it is installed independently of the application and MCP server registration. The [skills.sh directory](https://skills.sh/docs/faq) discovers public repository skills through installation telemetry; there is no separate package upload.

1. Update `metadata.version` in `SKILL.md`, the installation tag in README, and `skills/CHANGELOG.md`. Record the application interface targeted by the release in the setup guide. Keep all installed references and license texts inside the skill directory.
2. Validate frontmatter and relative references. Test installation from the candidate checkout with a pinned CLI version in a temporary project, selecting the supported agents. Set `DISABLE_TELEMETRY=1` for test installations. Confirm the installed files match the candidate, including both license texts. Exercise the documented workflows against the published application build; record the tested version/platform and any untested integration layers.
3. Open a skill-only PR and wait for every required check on its exact head. Merge with an expected-head check. Do not invoke the application release flow for a skill-only release.
4. At the verified merged commit, create an annotated `skill-vVERSION` tag matching `metadata.version`. The `skill-v*` namespace must reject tag updates and deletion. Check for an existing tag first; published tags are never moved or recreated.
5. Push that tag and install from its public URL into a fresh temporary project:

   ```sh
   DISABLE_TELEMETRY=1 npx skills@1.7.0 add \
     https://github.com/LorNtz/fluxcope/tree/skill-vVERSION/skills/fluxcope-skill \
     --agent codex claude-code --yes
   ```

   Replace `VERSION` with the released skill version. Verify its metadata, reference files, license texts, and agent discovery paths against the tagged source.
6. Publish GitHub release notes with the source commit, compatibility, validation results, and install command. Use `gh release create --verify-tag --latest=false` so the application's Latest release stays unchanged. Verify the release and Latest tag afterward. Directory indexing can lag behind successful installation; report those states separately.

The tested installer version is 1.7.0. Review its source/ref handling before changing that pin. A tag-pinned installation stays on that ref; users select another tag explicitly to upgrade or roll back. Corrections receive a new skill version and tag.
