# Brand registry

A brand registry is a JSON document that creates or updates mail profiles
(companies), assigns existing accounts to them, and defines the pinned
business tabs. Import it from Settings → Accounts → Import brand registry.
On launch, Flectar also applies `%APPDATA%\flectar-mail\brand-registry.json`
(the data directory on every platform) when that file is present and its
bytes have changed.

Import is idempotent. It never deletes accounts or mail. An account that no
rule names stays in the profile it already has. Applying the same file twice
changes nothing.

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
file. PNG, JPEG, GIF, and WebP up to 2 MB are stored under
`profile-logos/<id>.<ext>`. A logo that cannot be read is skipped.

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
stays where it is. The generator's "fallback to Tim Personal" is not applied.

### Tabs

`tabs[]` with `"builtin": true` (All Accounts) is skipped. Every other tab is
created or updated and shown, in file order, after All accounts. Tabs already
saved but absent from the file stay at the end.

| `rule` | Scope | What the mail view shows |
|---|---|---|
| `recipient` | `recipients` | Mail in any account whose To, Cc, or mailbox address matches `recipientPatterns` or `recipientAddresses` |
| `profiles` | `profiles` | Mail in the accounts of `profiles` (those values are profile ids or names) |
| `profiles+recipient+sender` | `any` | Mail in those accounts, **or** mail matching `extraRecipientAddresses`, **or** a sender rule |

`timAddresses` is merged into the Tim Mentink tab. Patterns use `*` and `?`.

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

The tab's logo and color come from its first profile. Message rows keep the
account's own profile color bar and logo.

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
