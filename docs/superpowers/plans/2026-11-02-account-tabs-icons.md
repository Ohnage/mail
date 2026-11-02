# Account, tab and application icons implementation plan

> **For agentic workers:** Use superpowers:subagent-driven-development or executing-plans. Work locally only.

**Goal:** Update the fork to upstream and deliver independent account/tab branding, editable account views, and the supplied application identity.

**Architecture:** Preserve the fork's existing profile and recipient/sender rule model. Add account icon overrides and optional explicit tab account membership, independently persisted from registry defaults. Share account icon resolution across surfaces; tab icons identify only the top navigation tab.

**Tech Stack:** Rust, Slint 1.18.1, SQLite settings, Pillow for deterministic app asset generation.

**Spec:** Approved design below, accepted by the user on 2026-11-02 at 06:10:46 UTC. The source PRD was read in full; these later instructions supersede conflicting profile/tab branding requirements.

## Global constraints and approved design

- Local checkout and verification only. No push, remote merge, PR, publication, deployment, installation, recurring updater, credentials, or live mail data/settings changes.
- Keep main-only project workflow in this isolated fresh checkout.
- Each account gets an uploaded icon used consistently in settings, sidebar and inbox rows.
- Each tab has its own name, icon and searchable account checklist. Accounts may appear in multiple tabs without moving mail.
- Preserve existing Tim/Forty7 recipient/sender filtering and memberships through migration and ordinary edits.
- Local customization survives registry imports and app updates.
- Supplied NewFlectarIcon.png replaces app identity without stretching or redesigning the artwork.
- Integrate current upstream first; check upstream again before final build and record exact SHA.
- Use synthetic/copied fixtures only. Complete coherent portions, then focused checks; broad checks at integration milestones.

## Review focus

- Missing versus explicitly empty membership must remain distinct; duplicate account IDs must not duplicate mail.
- Existing rule tabs must retain special scopes after rename/icon edits.
- Changed registries must retain local tabs, icon choices, names, memberships, ordering and deletions.
- Failed/cancelled image saves must leave previous values intact; large decoded dimensions must normalize safely.
- Long tab names, hundreds of accounts, narrow windows and high DPI must keep controls accessible.

## Tasks

1. **Upstream integration.** Resolve Rust, Slint and catalog conflicts preserving both sides; review auto-merged settings/schema/projection paths. Record fork/upstream SHAs and local merge commit. Run format/platform checks and core tests against synthetic fixtures; report tooling blockers honestly.
2. **Persistent model and registry safety.** Modify core models, lib and registry import. Add account icons; independent tab icon and optional account IDs (None preserves inherited membership; Some(empty) stays empty). Track local customization and preserve edited/deleted/reordered tabs on import. Add regression tests for persistence, scope preservation, many-to-many views and reimport behavior.
3. **Account and tab editing.** Modify settings/account/sidebar/message controllers and Slint models/dialog/navigation. Add normalized image upload utility, shared account-icon resolver, and atomic draft tab editor with searchable multi-select accounts. Reuse upstream visual components; replace clipped fixed coordinates with layouts and bounded navigation. Check selection filtering and runtime rendering with synthetic data.
4. **Application identity.** Preserve source PNG with provenance; update deterministic generator/default images/ICO and runtime consumers. Keep legacy appIcon deserialization but resolve variants to supplied design. Verify every ICO size and source aspect ratio; run generator check.
5. **Integration verification and deliverables.** Recheck upstream, run project format/platform/icon/core/workspace checks and Windows build if available. Inspect light/dark synthetic renders and settings/tab controls, build portable/installer artifact if supported. Record exact commands/results, SHAs, local artifacts and unresolved blockers; no installation or remote writes.

## Progress

- Fresh isolated checkout created at task/mail; main originally clean at 37e4fc94d621c995c9271fb397be978b1743509a.
- Upstream captured at 917ebd1f84eff7818e0a89bacd5e120e45b18b63; local merge conflict resolution in progress.
- Existing Rust 1.99.0 and Visual Studio Build Tools discovered; no tooling installed.
