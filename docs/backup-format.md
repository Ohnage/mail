# Account backup

Settings → export writes a JSON file. Import reads the same file. Credentials
are not included. Mail already stored on this computer is not deleted, and an
account whose address is already connected is left as it is.

## Envelope

```json
{
  "format": "flectar-mail-account-backup",
  "version": 2,
  "createdAt": "2026-11-01T18:00:00Z",
  "includesCredentials": false,
  "accounts": [],
  "preferences": {}
}
```

`format` must be `flectar-mail-account-backup`. `version` is `2`, or `1` for a
file written before connection security was stored explicitly. The file must
be 1 MB or smaller.

## Accounts

Each entry of `accounts` is the connection, without a password or token:

| Field | Meaning |
|---|---|
| `email` | Mailbox address. This is how a restored account is recognized. |
| `displayName` | Optional name. |
| `provider` | `gmail`, `microsoft`, `generic`, or another stored provider id. |
| `authKind` | `password` or `oauth`. The secret itself is not in the file. |
| `mailProtocol` | `imap`, `pop3`, `jmap`, or `gmail`. |
| `username` | Login name when it differs from `email`. |
| `jmapUrl` | JMAP session URL. Empty for other protocols. |
| `incomingHost`, `incomingPort` | Incoming server. |
| `smtpHost`, `smtpPort` | Submission server. |
| `settings` | Stored account settings, including `connection.incomingSecurity` and `connection.smtpSecurity` (`tls`, `starttls`, or `none`). |

Version 1 used `imapHost` / `imapPort`, the provider value `imap` for a
generic server, and could omit security. Import converts those the same way
the database migration does.

## Preferences

`preferences` is optional. When it is present, these fields are restored:

| Field | Meaning |
|---|---|
| `theme` | `system`, `light` / `snow`, or `dark` / `carbon`. |
| `messageAppearance` | `system`, `light`, or `dark`. |
| `showAvatars` | Sender images in the message list. |
| `workspaceLayout` | `default` or `minimal`. |
| `workspaceListPaneWidth` | List pane width in logical pixels. |
| `monochromeSidebarIcons` | Neutral sidebar icons. |
| `language` | `system` or a language code. |
| `calendarWeekStart` | `sunday` or `monday`. |
| `loadRemoteImages` | Load remote images in messages. |
| `markReadOnOpen` | Mark a message read when it is opened. |
| `collectOutgoingContacts`, `collectIncomingContacts` | Contact learning. |
| `contactSuggestAllAccounts`, `suggestLearnedContacts` | Suggestion scope. |
| `notificationsEnabled`, `notificationScope`, `soundEnabled` | Notifications. |
| `syncIntervalMinutes` | `1`, `5`, or `15`. |
| `closeToTray`, `windowFrame` | Window behavior. |
| `showAccountMarkers` | Color bar on the left of each message row. |
| `accountColors` | Map of email address to `#RRGGBB`. Every address must be one of the accounts in the file. |
| `mailProfiles` | Profiles. Each entry is `{ "name", "color", "accountEmails" }`. `color` is `#RRGGBB`. Every address must be in the file, and an address is in at most one profile. |

A backup with no `mailProfiles` field leaves existing profiles alone. `"mailProfiles": []` clears them. Profile logos, independent account icons, and business tabs are not part
of this account-connection backup. The application database stores
`accountIcons`, `businessTabs` (including each tab's independent `icon` and
optional `accountIds`), `businessTabsCustomized`, and the one-time icon
migration marker `businessTabIconsInitialized`; the referenced PNGs live
in `account-icons/` and `tab-icons/` under the application data directory.
Preserve both the database and those folders when copying these local
customizations to another installation. Local numeric account IDs are not
portable account identities and are deliberately not exported in this format.

The brand registry supplies default profiles and tabs. A registry import
stores the previous profiles and tabs as `brandRegistryBackup`, but that is
not a copy of icon files or an export of local customization. Once a tab has
been created, edited, deleted, or reordered, imports preserve the entire
customized list, including deletions and order, while refreshing registry
profiles. Account icon overrides also survive imports.
