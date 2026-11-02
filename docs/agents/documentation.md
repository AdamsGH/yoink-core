# Documentation maintenance

## Audience and location

- Keep public architecture, user, operator, and contributor documentation in core `docs/`.
- Put agent instructions, modification constraints, and agent-specific integration guidance under `docs/agents/`.
- Keep root and component READMEs human-facing. Describe purpose, current ownership, usage, and links.
- Use a topic directory when that topic needs more than two Markdown files.
- Do not add task diaries, worktree handoffs, temporary plans, or historical test totals to maintained documentation.
- Keep historical decisions explicitly historical. Link their current replacement.

## Accuracy

- Check claims against the current implementation before writing or moving them.
- Check commands against recipes and scripts. Check configuration against its actual loader and Compose overrides.
- Check API paths, response behavior, and authorization against native routes and tests.
- Distinguish bundle, engine, toolchain, and protocol versions.
- Distinguish standalone defaults from deployment overrides.
- Distinguish current native behavior from retained Python/PostgreSQL reference code.
- Preserve limits, uncertainty, safety conditions, and rollback constraints.
- Do not turn synthetic acceptance into a live-provider, production, security, or performance claim.
- Fix affected links and component READMEs in the same change.

## Writing

- Write artifacts in English unless the user requests another language.
- Use active voice and plain technical terms. Define uncommon terms once.
- Keep instructions to 20 words per sentence where precision permits. Keep descriptions to 25 words.
- Use one instruction per sentence. Use lists for sequences of three or more steps.
- Preserve qualifiers and modality. Do not shorten a sentence by changing its meaning.
- Avoid marketing adjectives, semicolons, typographic dashes, and decorative content.
- Keep each prose paragraph on one physical line. Preserve structural Markdown line breaks.
- Apply STE-inspired structural rules without claiming certified dictionary compliance.

## Verification and publication

1. Read the affected source before rewriting its documentation.
2. Check accuracy after each topic move or rewrite.
3. Update `docs/navigation.json` for public pages.
4. Run `just test-docs` and `just check-docs`.
5. Review the complete diff and submodule READMEs.
6. Generate and inspect the Wiki export with `just wiki`.
7. Confirm that no agent page or agent link enters the Wiki.
8. Recheck documented release commands before publication.
9. Report repository commits, image publication, deployment, and Wiki publication as separate outcomes.

Read this file before any public release, package/image publication, or Wiki publication. A successful link check does not replace the source-accuracy review.

Keep local `AGENTS.md` ignored and untracked. Do not copy personal policies or runtime state into public documentation.
