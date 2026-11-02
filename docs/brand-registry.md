# Brand registry

A brand registry is a JSON document that creates or updates mail profiles
(companies), assigns existing accounts to them, and defines the pinned
business tabs. Import it from Settings → Accounts → Import brand registry.
On launch, Flectar also applies `%APPDATA%\flectar-mail\brand-registry.json`
(the data directory on every platform) when that file is present and its
bytes have changed.

Import replaces mail profiles and untouched default business tabs. Locally
customized tab lists and independent account icons are preserved. Saved tab
lists from versions before customization tracking are conservatively treated
as customized, including an explicitly empty list. Older settings with no
`businessTabs` field remain eligible for the first registry seed. It never
deletes accounts or mail. Profile ids are the registry ids: a profile is not
matched by name onto an older id. An account no rule places goes to an
Unassigned profile. The profiles and tabs from before that import are stored
once in application settings as `brandRegistryBackup`, and a later import
does not overwrite that copy. Applying the same file twice changes nothing
after the first replace.

The file is hashed together with the importer version
(`flectar-brand-importer/2`). A newer importer re-applies a file whose bytes
have not changed.

## Accepted document

`schema` may be `flectar.brand-registry/v1-draft` (Tim's registry) or omitted.
A numeric `version` of `1` is the small form below. Unknown fields are ignored.
The file must be 8 MB or smaller.

### Profiles

```json
{
  "id": "forty7",
  "name": "Forty7 / Project 47",
  "color": "#222222",
  "logo": { "file": "logos/forty7.png" },
  "match": {
    "domains": ["p47.io", "forty7.io"],
    "addresses": ["47@mentinkizm.com"]
  }
}
```

`id` is kept when it is a slug (`letters`, digits, `-`, `_`). `color` is
`#RRGGBB`. `logo` is a `data:image/...;base64,...` URL, a filesystem path, or
an object whose `file` is that path. Paths are read relative to the registry
file, then from `brand-registry-logos/` in the data directory (a manual
import copies a sibling `logos/` directory there), then from the data
directory itself. PNG, JPEG, GIF, and WebP up to 2 MB are stored under
`profile-logos/<id>.<ext>`, replacing a logo already stored for that id.
When the registry file cannot be read, the previous logo for that id, or
else for that profile name, is written under the registry id.

### Which account moves

Rules are tried in this order. The first match wins.

1. `match.addresses` — the mailbox address, exact, across every profile.
2. Top-level `accounts[]` entries, `{ "email", "profile" }`, exact. This is
   how hub mailboxes are assigned when their domains are not listed.
3. String entries in `profiles[].accounts` (the small form), exact.
4. `match.domains`. A token that is not a domain is ignored, including
   `"<all 273 hub domains + niche domains>"`.
5. Wildcard entries in `profiles[].accounts` (`*` any run, `?` one character).

`gmail.com` on the Tim Mentink profile therefore claims a Gmail mailbox the
file does not name explicitly. A mailbox on a domain the file never mentions
goes to Unassigned. The generator's "fallback to Tim Personal" is not applied.

### Tabs

`tabs[]` with `"builtin": true` (All Accounts) is skipped. Every other tab
provides the default tabs, shown in file order after All accounts.
Until a tab is created, edited, deleted, or reordered locally, a changed
registry refreshes this default list. Once customized, the entire saved list
is authoritative: imports preserve its names, account selections, icons,
ordering, local additions, and deletions, including deletion of every tab.
Registry profile definitions and assignments still refresh. The original
profiles and tabs remain in `brandRegistryBackup`.

| `rule` | Scope | What the mail view shows |
|---|---|---|
| `recipient` | `recipients` | Mail in any account whose To, Cc, Bcc, or mailbox address matches `recipientPatterns` or `recipientAddresses`. This is not limited to the tab's profiles. |
| `profiles` | `profiles` | Mail in the accounts of `profiles` (those values are profile ids or names). An empty list matches nothing. |
| `profiles+recipient+sender` | `any` | Mail in those accounts, **or** mail matching `extraRecipientAddresses`, **or** a sender rule |

`timAddresses` is merged into the Tim Mentink tab. Patterns use `*` and `?`
and are matched without regard to case. Literal addresses are looked up in a
set, so a list of several hundred does not build an SQL `OR` tree.

Each account and each tab can have an independent normalized PNG icon.
`accountIcons` maps stringified local account IDs to `account-icons/*.png`;
`businessTabs[].icon` points to `tab-icons/*.png`. Registry imports preserve
these account overrides and all icons on a customized tab list. Existing
untouched tabs snapshot their old profile logo into a separate,
normalized tab icon on first settings load or registry import. This one-time
icon initialization also applies to conservatively protected legacy lists.
Registry refreshes retain existing independent tab icons by stable tab ID, so a later
profile logo upload cannot silently change tab branding. New tabs with no
icon use the generic tab image. Tab icons identify navigation only; account icons identify accounts in settings,
sidebars, and message rows.

A tab's optional `accountIds` supplies its independent account selection.
The same account may appear in several tabs. Missing `accountIds` retains
legacy profile-derived membership; an explicit empty array selects no
accounts. Selecting accounts never changes the account's profile or moves
mail. Editing the name, icon, or account list preserves the original scope,
profile IDs, recipient patterns, sender addresses, sender names, and sender
domains, including the Tim and Forty7 rules. The sidebar
lists only the tab's accounts. A recipient tab lists every account, collapsed
until that account is expanded; the choice is remembered per tab.

Sender rules live on `agentSenders`:

```json
"agentSenders": {
  "addresses": ["47@mentinkizm.com", "hi@p47.io"],
  "displayNames": ["Mailroom", "47 by Rook", "Rook", "Hermes"]
}
```

An address matches `From` exactly (or as a pattern when it contains `*` or
`?`). A display name matches only when the sender's domain is one of the
registry's own domains, so a stranger named "Rook" does not appear in Forty7.

## Small form

A shorter file is still accepted:

```json
{
  "version": 1,
  "profiles": [
    {
      "name": "Mentinkizm LLC",
      "color": "#EA580C",
      "logo": "data:image/png;base64,iVBORw0KGgo=",
      "accounts": ["owner@mentinkizm.com", "*@mentinkizm.com"]
    }
  ],
  "timAddresses": ["tim@*", "tmentink@gmail.com"],
  "tabs": [
    { "name": "Tim Mentink", "recipients": ["tim@*"] },
    { "name": "Mentinkizm", "profiles": ["Mentinkizm LLC"] }
  ]
}
```

An explicit address still beats a wildcard. A tab that names both profiles
and recipients keeps both constraints (accounts in those profiles, and the
recipient filter).

`docs/fixtures/brand-registry/registry.json` is a trimmed copy of the real
registry, and `docs/brand-registry.example.json` is the same example.
