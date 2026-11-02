Windows account organization correction
=======================================

Accounts are named inboxes (for example Demanaged), each containing any selected
connected mailboxes and/or receiving addresses. Business tabs select those named
accounts. Membership is many-to-many. Provider connections remain separate and
continue to own synchronization, sending permissions and message actions.

1. Remove the invalid 20,000-address discovery ceiling and stop refreshing provider
   suggestions as a prerequisite of unrelated settings saves. Reproduce with a
   synthetic 28,663-address catalog. Refresh suggestions explicitly at startup.
2. Seed named inbox accounts from existing branding profiles. Preserve existing
   tab selections/rules until membership is deliberately edited; keep user-owned
   accounts and icons across registry refreshes. Store changes atomically.
3. Accounts edit their name, icon, connected mailbox/alias selections; tabs edit
   their identity and named-account checklist. Use virtualized, searchable lists.
   Individual account scopes and tab unions must use the same recipient filters.
4. Integrate upstream, compile and test on Windows, inspect synthetic light/dark
   layouts, publish a normal fast-forward and verify the Windows installer.

Never run a live data migration or installation as part of development. Validate
against synthetic databases. Preserve the supplied application/installer artwork.
