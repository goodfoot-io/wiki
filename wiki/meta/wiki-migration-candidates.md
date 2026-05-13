---
title: Wiki Migration Candidates
summary: Markdown files across the repository that are good candidates for migration into wiki/ or conversion to *.wiki.md files, with reasoning for each.
tags:
  - meta
  - wiki
  - maintenance
---

Living list of candidate files for wiki migration. Files are grouped by source area. Each entry notes the recommended destination (wiki/ page vs embedded *.wiki.md) and brief reasoning.

See [Wiki Organization](./wiki-organization.md) for the embed vs. centralize decision criteria.

---

## documentation/ (root-level files)

- **cards-extension.md** — `wiki/architecture/cards-system-design.md`
  Reason: Cross-cutting architecture document explaining Cards' core abstractions (attention tracking, plan approval gates, git-backed storage, REST API). Synthesizes design patterns across multiple packages.

- **claude-code-cli-style-guide.md** — `wiki/guides/skill-markdown-style-guide.md`
  Reason: Recurring reference for contributors writing SKILL.md and plugin markdown. Standardizes formatting conventions across the codebase.

- **commanders-intent-in-planning.md** — `wiki/guides/commanders-intent-in-card-planning.md`
  Reason: Durable explanation of planning methodology grounded in military doctrine, applied to how cards are planned. Explains "why" behind planning practices used across the system.

- **e2e-git-testing.md** — `wiki/architecture/e2e-git-hook-testing.md`
  Reason: Cross-cutting test infrastructure architecture. Explains full subprocess chain from git operation through cards API. Relevant to multiple packages.

- ~~**edh-template-update-procedure.md**~~ — migrated (target page in external repo)

- **icon-server.md** — `wiki/architecture/icon-server-webview-api.md`
  Reason: API specification and design rationale for solving webview icon theme access problem. Cross-cutting feature spanning extension host and webview communication.

- **inline-bash-and-claude-env-vars.md** — `wiki/reference/claude-plugin-bash-syntax.md`
  Reason: Catalog and reference for inline bash execution and Claude-specific variable substitution patterns used throughout plugin skills.

- **remote-connection-tunnels.md** — `wiki/reference/vscode-remote-tunnel-architecture.md`
  Reason: Technical explanation of VS Code's remote container channel pipe implementation. System-level understanding of infrastructure behavior.

- **stop-hook-output.md** — `wiki/architecture/card-commit-attribution-pipeline.md`
  Reason: Detailed explanation of how commits are attributed to cards via stop hooks. Cross-cutting system behavior involving multiple packages.

- **terminology.md** — `wiki/reference/compare-session-terminology.md`
  Reason: Reference glossary for CompareSessionManager and related UI terminology. Standardizes terminology across codebase.

- **visual-representations-of-code-examples.md** — `wiki/guides/commit-visualization-techniques.md`
  Reason: Guide to visualization techniques for representing code changes and commit metadata. Applicable across multiple UI contexts (timeline, diff views).

- **workspace-commits-attribution.md** — `wiki/architecture/workspace-commit-attribution-flow.md`
  Reason: Detailed explanation of how workspace commits flow into card attribution via API and stop hooks. Cross-cutting system behavior.

### Excluded

- **claude-bare-mode.md** — Ephemeral debugging configuration notes without lasting architectural value.
- **codex-runtime-plugin.md** — Specific to Codex configuration; represents point-in-time recommendation tied to single version.
- **jakob-nielsen.md** — Conceptual design principles essay; cannot be code-anchored; exists independent of codebase.
- **plan-evaluation-subagent-report.md** — Ephemeral session analysis report of specific planning sessions.
- **port-claude-plugin.md** — One-time project implementation plan; not durable knowledge across multiple commits.
- **qmd-enhanced-search-questions.md** — Test artifact (Q&A questions); not architectural or operational knowledge.
- **registration-completion-items.md** — Ephemeral completion checklist tied to specific point-in-time project status.
- **todo.md** — Bare TODO list; ephemeral task list with no lasting value.
- **tunnel-bug.md** — Resolved bug report; historical incident analysis without lasting architectural value.
- **tunnel-patch.md** — Resolved incident analysis explaining why patches don't work; historical documentation of investigation.
- **visual-representations-of-code.md** — Research survey of AST and code representation literature; not tied to this codebase's implementation.
- **wiki-index-plan.md** — One-time project implementation plan; not durable knowledge across multiple commits.

---

## documentation/ (subdirectories)

### Telemetry

- **documentation/telemetry/north-star.md** — `wiki/guides/telemetry-north-star.md`
  Reason: Explains the foundational principles and evidence framework for telemetry instrumentation in the Cards extension (activation, retention, health metrics, privacy constraints). This synthesizes requirements across the extension system and answers "why" the instrumentation strategy exists. Highly durable, relevant across multiple commits, and can be anchored to telemetry.json and sanitizer implementation.

- **documentation/telemetry/plan.md** — `packages/extension/TELEMETRY.wiki.md`
  Reason: Comprehensive taxonomy redesign for the Cards extension's telemetry system. Since it is primarily about extension-specific instrumentation (setup wizard, actions, API server) and references specific files like SetupWizardViewProvider.ts, it is better suited as an embedded document alongside the extension package where maintainers will encounter it directly, ensuring it stays current as the extension evolves.

### Excluded

**documentation/adverserial/agents/*.md** — Agent role definitions and prompts designed for skill invocation. Ephemeral infrastructure for a specific use case, not durable knowledge about the codebase. Cannot be anchored to source code.

**documentation/adverserial/skills/walkthrough/SKILL.md** — Skill definition and orchestration instructions. Part of the adversarial review infrastructure, not explanatory documentation about the codebase itself.

**documentation/telemetry/agents/*.md** — Agent prompts and role definitions for telemetry team members. Ephemeral instructions for agent invocation, not documentation about how the codebase works. These serve the skill system, not readers trying to understand the system.

**documentation/telemetry/commands/telemetry-team.md** — Skill command definition for launching the telemetry team. Infrastructure for agent orchestration, not codebase documentation.

**documentation/telemetry/roles/*.md** — Organizational role descriptions that define responsibilities within a telemetry team structure. While they describe important responsibilities, they are about team process and organizational structure rather than codebase architecture, and belong in operational documentation (team wiki, handbook, or org chart) rather than the code's architecture documentation.

---

## packages/ and other areas

- **packages/cards/web/SPACING.md** — `packages/cards/web/SPACING.wiki.md`
  Reason: UI design system specification with concrete spacing rules for the Cards component; local to the web package.

- **packages/extension/CLAUDE_TEST_CACHING.md** — `packages/extension/CLAUDE_TEST_CACHING.wiki.md`
  Reason: Technical guide explaining test caching behavior and management; component-specific developer reference.

- **packages/extension/docs/git-command-spec.md** — `wiki/architecture/extension-git-commands.md`
  Reason: Comprehensive reference documenting git commands used by the extension and their error semantics; cross-cutting reference for anyone maintaining extension git integration.

- **packages/extension/docs/registration-system.md** — `wiki/architecture/registration-system.md`
  Reason: End-to-end architecture documentation of the license registration system spanning website and extension; foundational for understanding a major subsystem.

- **packages/extension/docs/registration-system-dev-guide.md** — `wiki/guides/registration-system-development.md`
  Reason: Hands-on developer guide for working with registration system during development; cross-package workflow documentation.

- **packages/website/CHECKLIST.md** — `wiki/guides/registration-flow-testing-checklist.md`
  Reason: Operational checklist for validating end-to-end registration flow; test workflow that spans website and extension packages.

- **packages/website/CLAUDE.md** — `wiki/guides/website-development.md`
  Reason: Detailed dev server setup, database configuration, and authentication testing guide for website package; foundational developer guide.

- **public/packages/test-utils/CLAUDE.md** — `public/packages/test-utils/CLAUDE.wiki.md`
  Reason: Usage documentation for shared test utilities library; guidance embedded alongside the component.

- **public/plugins/runtime/docs/CONTEXT_MAP.md** — `wiki/architecture/runtime-plugin-agents.md`
  Reason: Map of runtime plugin agents, skills, and workflows; foundational architecture documentation for the runtime system.

- **public/plugins/runtime/docs/CONTEXT_MAP_GUIDE.md** — `wiki/guides/maintaining-runtime-context-map.md`
  Reason: Guide for keeping the runtime context map accurate as the plugin evolves; maintenance documentation for cross-cutting architecture.

### Excluded

- **packages/cards/server/CLAUDE.md** — Single-line note directing to plugin files; not substantive enough for wiki migration.
- **packages/extension/CLAUDE.md** — Minimal snippets about tests and VSCode difficulty; too thin for dedicated wiki page.
- **packages/extension/README.md** — Product marketing copy for VS Marketplace; external-facing promotional content, not internal documentation.
- **packages/extension/docs/gif-generation.md** — Single command snippet for GIF generation; too minimal as standalone page.
- **packages/extension/docs/marketplace-optimization.md** — External research report on marketplace positioning; one-time analysis, not documentation.
- **packages/extension/telemetry-data/DASHBOARD.md** — Telemetry dashboard metrics output; transient data, not documentation.
- **public/packages/claude-code-hooks-api/README.md** — Example project boilerplate; standard template content.
- **public/packages/claude-code-hooks-runtime/README.md** — Example project boilerplate; standard template content.
- **public/README.md** — Product marketing copy for VS Marketplace; external-facing content.
- **public/CHANGELOG.md** — Release notes and changelog; version-specific historical record.
- **public/SECURITY.md** — External-facing security policy; not internal documentation.
