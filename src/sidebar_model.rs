//! Visible sidebar projection and batched, retained Slint model updates.
use super::*;

pub(super) type SidebarModel = crate::retained_model::RetainedModel<SidebarRow>;
const MAX_ANIMATED_SIDEBAR_ROWS: usize = 256;

// Keep closing descendants in the virtualized model until their heights have
// sprung to zero. The canonical projection still contains only expanded rows.
#[derive(Default)]
pub(super) struct SidebarAnimation {
    target: RefCell<Vec<SidebarRow>>,
    generation: Cell<u64>,
}

impl SidebarAnimation {
    pub(super) fn reconcile(
        self: &Rc<Self>,
        model: &Rc<SidebarModel>,
        rows: Vec<SidebarRow>,
        animate: bool,
    ) {
        let previous = self.target.borrow();
        let changed = animate && accordion_changed(&previous, &rows);
        let changed_row_count = if changed {
            let previous_keys = previous
                .iter()
                .map(|row| row.key.as_str())
                .collect::<HashSet<_>>();
            let next_keys = rows
                .iter()
                .map(|row| row.key.as_str())
                .collect::<HashSet<_>>();
            previous_keys.symmetric_difference(&next_keys).count()
        } else {
            0
        };
        drop(previous);
        *self.target.borrow_mut() = rows.clone();
        // Only explicit accordion toggles animate. Search and data refreshes
        // replace the projection immediately, so stale rows cannot linger.
        // Large trees keep the retained model's batched range update path.
        if !changed || changed_row_count > MAX_ANIMATED_SIDEBAR_ROWS {
            self.generation.set(self.generation.get().wrapping_add(1));
            model.reconcile(rows);
            return;
        }
        let current = model.iter().collect::<Vec<_>>();
        let projected = transition_rows(current, rows, true);
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        model.reconcile(projected);
        let animation = Rc::downgrade(self);
        let model = Rc::downgrade(model);
        Timer::single_shot(Duration::from_millis(260), move || {
            if let (Some(animation), Some(model)) = (animation.upgrade(), model.upgrade())
                && animation.generation.get() == generation
            {
                model.reconcile(animation.target.borrow().clone());
            }
        });
    }
}

fn accordion_changed(before: &[SidebarRow], after: &[SidebarRow]) -> bool {
    let previous = before
        .iter()
        .map(|row| (row.key.as_str(), row))
        .collect::<HashMap<_, _>>();
    after.iter().any(|row| {
        previous.get(row.key.as_str()).is_some_and(|old| {
            (matches!(
                row.kind,
                SidebarRowKind::UnifiedSection
                    | SidebarRowKind::Account
                    | SidebarRowKind::CategoriesSection
                    | SidebarRowKind::GlobalLabelsSection
                    | SidebarRowKind::AccountLabelsSection
            ) && old.open != row.open)
                || (row.kind == SidebarRowKind::Folder
                    && old.mailbox.expanded != row.mailbox.expanded)
                || (row.kind == SidebarRowKind::AccountLabel
                    && old.label.expanded != row.label.expanded)
        })
    })
}

fn transition_rows(
    current: Vec<SidebarRow>,
    rows: Vec<SidebarRow>,
    reveal_new: bool,
) -> Vec<SidebarRow> {
    let current_keys = current
        .iter()
        .map(|row| row.key.clone())
        .collect::<HashSet<_>>();
    let target_keys = rows
        .iter()
        .map(|row| row.key.clone())
        .collect::<HashSet<_>>();
    let mut before = HashMap::<slint::SharedString, Vec<SidebarRow>>::new();
    let mut trailing = Vec::new();
    let mut next = None;
    for mut row in current.into_iter().rev() {
        if target_keys.contains(row.key.as_str()) {
            next = Some(row.key.clone());
        } else {
            row.show_row = false;
            row.reveal_row = false;
            if let Some(key) = &next {
                before.entry(key.clone()).or_default().push(row);
            } else {
                trailing.push(row);
            }
        }
    }
    let mut result = Vec::with_capacity(
        rows.len() + before.values().map(Vec::len).sum::<usize>() + trailing.len(),
    );
    for mut row in rows {
        if let Some(mut closing) = before.remove(row.key.as_str()) {
            closing.reverse();
            result.extend(closing);
        }
        row.reveal_row = reveal_new && !current_keys.contains(row.key.as_str());
        result.push(row);
    }
    trailing.reverse();
    result.extend(trailing);
    result
}

impl SidebarModel {
    fn reconcile(&self, rows: Vec<SidebarRow>) {
        self.reconcile_by(rows, |row| row.key.clone(), same_row);
    }
}

fn collapse_new_account_label_sections(
    account_ids: impl IntoIterator<Item = i64>,
    initialized: &mut HashSet<i64>,
    collapsed: &mut HashSet<String>,
    collapse_accounts: bool,
) {
    for account_id in account_ids {
        if initialized.insert(account_id) {
            collapsed.insert(format!("account-labels:{account_id}"));
            if collapse_accounts {
                collapsed.insert(format!("account:{account_id}"));
            }
        }
    }
}

fn collapse_new_folder_sections(
    folder_ids: impl IntoIterator<Item = i64>,
    initialized: &mut HashSet<i64>,
    collapsed: &mut HashSet<i64>,
) {
    for folder_id in folder_ids {
        if initialized.insert(folder_id) {
            collapsed.insert(folder_id);
        }
    }
}

// Slint image handles can compare unequal even when both are empty. Compare
// absent avatars semantically, otherwise every ordinary folder emits a change.
fn same_row(a: &SidebarRow, b: &SidebarRow) -> bool {
    let MailboxRow {
        account_id,
        folder_id,
        parent_folder_id,
        depth,
        has_children,
        expanded,
        is_standard,
        is_selectable,
        can_create_children,
        can_rename,
        can_delete,
        label_has_emoji,
        label,
        scope,
        context,
        detail,
        avatar,
        avatar_image,
        has_avatar,
        is_account,
        count,
        color,
        has_custom_color,
    } = &a.mailbox;
    let other = &b.mailbox;
    a.key == b.key
        && a.kind == b.kind
        && a.open == b.open
        && a.show_row == b.show_row
        && a.reveal_row == b.reveal_row
        && a.label == b.label
        && *account_id == other.account_id
        && *folder_id == other.folder_id
        && *parent_folder_id == other.parent_folder_id
        && *depth == other.depth
        && *has_children == other.has_children
        && *expanded == other.expanded
        && *is_standard == other.is_standard
        && *is_selectable == other.is_selectable
        && *can_create_children == other.can_create_children
        && *can_rename == other.can_rename
        && *can_delete == other.can_delete
        && *label_has_emoji == other.label_has_emoji
        && *label == other.label
        && *scope == other.scope
        && *context == other.context
        && *detail == other.detail
        && *avatar == other.avatar
        && *has_avatar == other.has_avatar
        && *is_account == other.is_account
        && *count == other.count
        && *color == other.color
        && *has_custom_color == other.has_custom_color
        && (!has_avatar || *avatar_image == other.avatar_image)
}

pub(super) fn refresh_sidebar(state: &Rc<RefCell<InboxState>>) {
    refresh_sidebar_with_motion(state, false);
}

pub(super) fn refresh_sidebar_with_motion(state: &Rc<RefCell<InboxState>>, animate: bool) {
    let mut state = state.borrow_mut();
    let account_ids = state
        .mailboxes
        .iter()
        .filter(|mailbox| mailbox.is_account)
        .map(|mailbox| mailbox.account_id)
        .collect::<Vec<_>>();
    let folder_ids = state
        .mailboxes
        .iter()
        .filter(|mailbox| !mailbox.is_account && mailbox.has_children && mailbox.folder_id >= 0)
        .map(|mailbox| mailbox.folder_id)
        .collect::<Vec<_>>();
    let collapse_accounts = !state.sidebar_collapse_tab.is_empty();
    let InboxState {
        initialized_sidebar_accounts,
        initialized_sidebar_folders,
        collapsed_sidebar_sections,
        collapsed_folder_ids,
        ..
    } = &mut *state;
    collapse_new_account_label_sections(
        account_ids,
        initialized_sidebar_accounts,
        collapsed_sidebar_sections,
        collapse_accounts,
    );
    collapse_new_folder_sections(
        folder_ids,
        initialized_sidebar_folders,
        collapsed_folder_ids,
    );
    let query = state.folder_filter.trim().to_owned();
    let filtering = !query.is_empty();
    let no_collapsed_folders = HashSet::new();
    let mut mailboxes = make_mailbox_rows(
        &state.mailboxes,
        &state.profile_avatar_images,
        &state.labels,
        if filtering {
            &no_collapsed_folders
        } else {
            &state.collapsed_folder_ids
        },
    );
    for row in &mut mailboxes {
        if row.is_account {
            if let Some(icon) = state
                .account_presentation
                .account_icon(i64::from(row.account_id))
            {
                row.avatar_image = icon.clone();
                row.has_avatar = true;
            }
            if let Some(color) = state
                .account_presentation
                .saved_marker(i64::from(row.account_id))
            {
                row.color = color;
                row.has_custom_color = true;
            }
        }
    }
    let account_names = mailboxes
        .iter()
        .filter(|mailbox| mailbox.is_account)
        .map(|mailbox| (mailbox.account_id, mailbox.label.clone()))
        .collect::<HashMap<_, _>>();
    let mut labels = make_label_rows(&state.labels, None, "");
    for label in &mut labels {
        if let Some(name) = account_names.get(&label.account_id) {
            label.account_name = name.clone();
        } else if label.is_global {
            label.account_name = "All accounts".into();
        }
    }
    let gmail_account_ids = state
        .connected_accounts
        .iter()
        .filter(|account| account.mail_protocol == MailProtocol::Gmail)
        .filter_map(|account| i32::try_from(account.id).ok())
        .collect::<HashSet<_>>();
    let business_tab = state.account_presentation.active_business().is_some();
    let unified_rows = make_mailbox_rows(
        &state.unified_mailboxes,
        &state.profile_avatar_images,
        &state.labels,
        &no_collapsed_folders,
    );
    let mut rows = if business_tab {
        let company_ids = state.company_account_ids.clone().unwrap_or_default();
        let tab = state.account_presentation.active_business().cloned();
        let tab_heading = tab
            .as_ref()
            .map(|tab| tab.name.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "Company".into());
        let folder_color = tab
            .as_ref()
            .map(|tab| state.account_presentation.business_tab_color(tab));
        let no_sections = HashSet::new();
        let no_folders = HashSet::new();
        let collapsed_sections = if filtering {
            &no_sections
        } else {
            &state.collapsed_sidebar_sections
        };
        let collapsed_folders = if filtering {
            &no_folders
        } else {
            &state.collapsed_folder_ids
        };
        let (company_rows, company_mailboxes) = make_company_sidebar_rows(
            mailboxes,
            unified_rows,
            labels,
            &state.labels,
            &gmail_account_ids,
            collapsed_sections,
            collapsed_folders,
            &company_ids,
            &state.folder_unreads,
            state.inbox_count,
            &tab_heading,
            folder_color,
            state
                .account_presentation
                .pinned_sidebar_tags
                .get(
                    &state
                        .account_presentation
                        .active_business_tab
                        .clone()
                        .unwrap_or_default(),
                )
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        );
        state.company_mailboxes = company_mailboxes;
        if filtering {
            filter_company_rows(company_rows, &query)
        } else {
            company_rows
        }
    } else if filtering {
        state.company_mailboxes.clear();
        make_filtered_sidebar_rows(mailboxes, labels, &query, &gmail_account_ids)
    } else {
        state.company_mailboxes.clear();
        make_sidebar_rows(
            SidebarLayout::for_account_count(state.mail_accounts().len()),
            mailboxes,
            unified_rows,
            labels,
            &gmail_account_ids,
            &state.collapsed_sidebar_sections,
            state
                .local_mail
                .and_then(|local| i32::try_from(local.account_id).ok()),
        )
    };
    let address_rows = state
        .account_presentation
        .visible_inbox_accounts()
        .into_iter()
        .filter(|account| !filtering || account.name.to_lowercase().contains(&query.to_lowercase()))
        .map(|account| {
            let choice = state.account_presentation.inbox_choice(account);
            let mut entry = row(
                SidebarRowKind::AddressInbox,
                format!("inbox-account:{}", account.id),
            );
            let marker = state.account_presentation.saved_inbox_color(account);
            entry.mailbox = MailboxRow {
                account_id: -1,
                folder_id: -1,
                scope: Scope::AddressInbox(account.id).to_string().into(),
                label: choice.name,
                detail: choice.detail,
                avatar: choice.initials,
                avatar_image: choice.avatar,
                has_avatar: choice.has_avatar,
                color: marker.unwrap_or_default(),
                has_custom_color: marker.is_some(),
                count: if state.scope == Scope::AddressInbox(account.id) && state.inbox_count > 0 {
                    state.inbox_count.to_string().into()
                } else {
                    Default::default()
                },
                ..Default::default()
            };
            entry
        })
        .collect::<Vec<_>>();
    // Named accounts replace the raw connection list, except on a business
    // tab whose chosen accounts span mailboxes: those mailboxes' Inbox, Sent,
    // and folders have to stay reachable.
    let tab_active = state.account_presentation.active_business().is_some();
    let tab_has_mailboxes = state
        .profile_accounts
        .as_ref()
        .is_some_and(|ids| !ids.is_empty());
    if hides_connection_rows(
        !state.account_presentation.inbox_accounts.is_empty(),
        tab_active && tab_has_mailboxes,
    ) {
        rows.retain(|row| {
            !matches!(
                row.kind,
                SidebarRowKind::Account
                    | SidebarRowKind::AccountLabelsSection
                    | SidebarRowKind::AccountLabel
            ) && row.mailbox.account_id <= 0
        });
    }
    let rows = place_named_account_rows(business_tab, rows, address_rows);
    // Model notifications can synchronously inspect application state.
    let model = Rc::clone(&state.sidebar_rows);
    let animation = Rc::clone(&state.sidebar_animation);
    drop(state);
    animation.reconcile(&model, rows, animate);
}

/// On a business tab the named accounts sit directly under Company Inbox.
/// Elsewhere they stay at the top of the sidebar.
fn place_named_account_rows(
    business_tab: bool,
    mut rows: Vec<SidebarRow>,
    mut address_rows: Vec<SidebarRow>,
) -> Vec<SidebarRow> {
    if !business_tab {
        address_rows.append(&mut rows);
        return address_rows;
    }
    let mut merged = Vec::with_capacity(rows.len() + address_rows.len());
    let mut placed = false;
    for row in rows {
        let inbox = row.kind == SidebarRowKind::UnifiedInbox && row.key.as_str() == "company-inbox";
        merged.push(row);
        if inbox {
            merged.append(&mut address_rows);
            placed = true;
        }
    }
    if !placed {
        merged.append(&mut address_rows);
    }
    merged
}

fn make_filtered_sidebar_rows(
    mailboxes: Vec<MailboxRow>,
    labels: Vec<MailLabelRow>,
    query: &str,
    gmail_account_ids: &HashSet<i32>,
) -> Vec<SidebarRow> {
    use SidebarRowKind as Kind;

    let query = query.to_lowercase();
    let mut accounts = Vec::new();
    let mut folders: HashMap<i32, Vec<MailboxRow>> = HashMap::new();
    for mailbox in mailboxes {
        if mailbox.is_account {
            accounts.push(mailbox);
        } else if mailbox.label.to_lowercase().contains(&query) {
            folders.entry(mailbox.account_id).or_default().push(mailbox);
        }
    }

    let mut account_labels: HashMap<i32, Vec<MailLabelRow>> = HashMap::new();
    let mut global_labels = Vec::new();
    let mut categories = Vec::new();
    for mut label in labels
        .into_iter()
        .filter(|label| label.name.to_lowercase().contains(&query))
    {
        label.display_name = label.name.clone();
        label.depth = 0;
        label.has_children = false;
        label.expanded = true;
        label.can_create_children = gmail_account_ids.contains(&label.account_id);
        if label.is_auto {
            categories.push(label);
        } else if label.is_global {
            global_labels.push(label);
        } else {
            account_labels
                .entry(label.account_id)
                .or_default()
                .push(label);
        }
    }

    let mut rows = Vec::new();
    if !categories.is_empty() {
        rows.push(section(
            Kind::CategoriesSection,
            "categories",
            &HashSet::new(),
        ));
        rows.extend(categories.into_iter().map(|label| SidebarRow {
            key: format!("category:{}", label.id).into(),
            label,
            ..row(Kind::Category, "")
        }));
    }
    if !global_labels.is_empty() {
        rows.push(section(
            Kind::GlobalLabelsSection,
            "global-labels",
            &HashSet::new(),
        ));
        rows.extend(global_labels.into_iter().map(|label| SidebarRow {
            key: format!("global-label:{}", label.id).into(),
            label,
            ..row(Kind::GlobalLabel, "")
        }));
    }
    for account in accounts {
        let account_id = account.account_id;
        let matches = folders.remove(&account_id).unwrap_or_default();
        let label_matches = account_labels.remove(&account_id).unwrap_or_default();
        if matches.is_empty() && label_matches.is_empty() {
            continue;
        }
        rows.push(SidebarRow {
            open: true,
            mailbox: account,
            ..row(Kind::Account, format!("account:{account_id}"))
        });
        rows.extend(matches.into_iter().map(|mut mailbox| {
            // Search results are a flat projection; their original hierarchy
            // remains untouched and returns when the filter is cleared.
            mailbox.depth = 0;
            mailbox.has_children = false;
            let key = format!("mailbox:{account_id}:{}", mailbox.scope);
            SidebarRow {
                mailbox,
                ..row(Kind::Folder, key)
            }
        }));
        if !label_matches.is_empty() {
            rows.push(SidebarRow {
                mailbox: MailboxRow {
                    account_id,
                    ..Default::default()
                },
                open: true,
                ..row(
                    Kind::AccountLabelsSection,
                    format!("account-labels:{account_id}"),
                )
            });
            rows.extend(label_matches.into_iter().map(|label| SidebarRow {
                key: format!("account-label:{account_id}:{}", label.id).into(),
                label,
                ..row(Kind::AccountLabel, "")
            }));
        }
    }
    rows
}

fn row(kind: SidebarRowKind, key: impl Into<slint::SharedString>) -> SidebarRow {
    SidebarRow {
        kind,
        key: key.into(),
        show_row: true,
        ..Default::default()
    }
}

fn section(kind: SidebarRowKind, key: &str, collapsed: &HashSet<String>) -> SidebarRow {
    SidebarRow {
        open: !collapsed.contains(key),
        ..row(kind, key)
    }
}

fn account_label_rows(
    labels: &[MailLabelRow],
    account_id: i32,
    nested: bool,
    collapsed: &HashSet<String>,
) -> Vec<SidebarRow> {
    use SidebarRowKind as Kind;

    let account_labels = labels
        .iter()
        .filter(|label| label.account_id == account_id)
        .cloned()
        .collect::<Vec<_>>();
    if !nested {
        return account_labels
            .into_iter()
            .map(|mut label| {
                label.display_name = label.name.clone();
                SidebarRow {
                    key: format!("account-label:{account_id}:{}", label.id).into(),
                    label,
                    ..row(Kind::AccountLabel, "")
                }
            })
            .collect();
    }

    let ids_by_name = account_labels
        .iter()
        .map(|label| (label.name.to_string(), label.id))
        .collect::<HashMap<_, _>>();
    let mut children: HashMap<Option<i32>, Vec<MailLabelRow>> = HashMap::new();
    for label in account_labels {
        let parent_id = label
            .name
            .rsplit_once('/')
            .and_then(|(parent, _)| ids_by_name.get(parent).copied());
        children.entry(parent_id).or_default().push(label);
    }

    let mut stack = children
        .remove(&None)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .map(|label| (label, 0))
        .collect::<Vec<_>>();
    let mut rows = Vec::new();
    while let Some((mut label, depth)) = stack.pop() {
        let key = format!("gmail-label:{account_id}:{}", label.id);
        let descendants = children.remove(&Some(label.id)).unwrap_or_default();
        label.display_name = if depth > 0 {
            label
                .name
                .rsplit_once('/')
                .filter(|(_, leaf)| !leaf.is_empty())
                .map(|(_, leaf)| leaf.into())
                .unwrap_or_else(|| label.name.clone())
        } else {
            label.name.clone()
        };
        label.depth = depth;
        label.has_children = !descendants.is_empty();
        label.expanded = !collapsed.contains(&key);
        label.can_create_children = true;
        rows.push(SidebarRow {
            key: key.clone().into(),
            label,
            ..row(Kind::AccountLabel, "")
        });
        if !collapsed.contains(&key) {
            stack.extend(
                descendants
                    .into_iter()
                    .rev()
                    .map(|child| (child, depth.saturating_add(1))),
            );
        }
    }
    rows
}

/// Whether the sidebar leads with cross-account shortcuts. With one account
/// they would only duplicate that account's own mailboxes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SidebarLayout {
    Unified,
    SingleAccount,
}

impl SidebarLayout {
    /// Every connected account counts, including ones whose folders have not
    /// been discovered yet or that are currently offline.
    pub(super) fn for_account_count(count: usize) -> Self {
        if count > 1 {
            Self::Unified
        } else {
            Self::SingleAccount
        }
    }
}

/// Unified layout: shortcuts, shared navigation, then accounts. Single-account
/// layout: the account first, then the shared categories and labels that
/// remain reachable without the unified section.
fn make_sidebar_rows(
    layout: SidebarLayout,
    mailboxes: Vec<MailboxRow>,
    unified: Vec<MailboxRow>,
    labels: Vec<MailLabelRow>,
    gmail_account_ids: &HashSet<i32>,
    collapsed: &HashSet<String>,
    local_mail: Option<i32>,
) -> Vec<SidebarRow> {
    let shared = shared_navigation_rows(&labels, collapsed);
    let accounts = account_rows(mailboxes, &labels, gmail_account_ids, collapsed);
    match layout {
        SidebarLayout::Unified => {
            let mut rows = unified_rows(unified);
            rows.extend(shared);
            rows.push(row(SidebarRowKind::AccountsHeading, "accounts-heading"));
            rows.extend(accounts);
            rows
        }
        // Mail kept on this device comes last, after what the account shares.
        SidebarLayout::SingleAccount => {
            let (local, mut rows): (Vec<_>, Vec<_>) = accounts
                .into_iter()
                .partition(|row| Some(row.mailbox.account_id) == local_mail);
            rows.extend(shared);
            rows.extend(local);
            rows
        }
    }
}

fn company_unified_rank(scope: &str) -> u8 {
    match scope {
        "Unified Sent" => 0,
        "Unified Drafts" => 1,
        "Unified Archive" => 2,
        "Unified Spam" => 3,
        "Unified Trash" => 4,
        "Unified Starred" => 5,
        _ => 6,
    }
}

fn label_swatch(value: &str) -> slint::Color {
    value
        .strip_prefix('#')
        .filter(|hex| hex.len() == 6)
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
        .map(|rgb| {
            slint::Color::from_rgb_u8(
                ((rgb >> 16) & 0xff) as u8,
                ((rgb >> 8) & 0xff) as u8,
                (rgb & 0xff) as u8,
            )
        })
        .unwrap_or_else(|| slint::Color::from_rgb_u8(107, 114, 128))
}

fn company_tag_copies(
    source_labels: &[flectar_mail_core::models::Label],
    company: &HashSet<i64>,
) -> Vec<flectar_mail_core::company_view::TagCopy> {
    source_labels
        .iter()
        .filter(|label| {
            !label.is_auto
                && label
                    .owner_account_id
                    .is_some_and(|account_id| company.contains(&account_id))
                && !label.keyword.is_empty()
        })
        .map(|label| flectar_mail_core::company_view::TagCopy {
            account_id: label.owner_account_id.unwrap_or_default(),
            label_id: label.id,
            keyword: label.keyword.clone(),
            name: label.name.clone(),
            color: label.color.clone(),
        })
        .collect()
}

fn pinned_tag_rows(
    tags: &[flectar_mail_core::company_view::MergedTag],
    pinned: &[String],
) -> Vec<SidebarRow> {
    use SidebarRowKind as Kind;
    let mut rows = Vec::new();
    for keyword in pinned {
        let key = flectar_mail_core::company_view::tag_key(keyword);
        let Some(tag) = tags.iter().find(|tag| {
            flectar_mail_core::company_view::tag_key(&tag.keyword) == key
        }) else {
            continue;
        };
        let Some(member) = tag.members.first() else {
            continue;
        };
        let Some(id) = i32::try_from(member.label_id).ok() else {
            continue;
        };
        let stored = tag
            .members
            .iter()
            .map(|member| member.color.as_str())
            .collect::<Vec<_>>();
        let (color, icon_kind) =
            flectar_mail_core::company_view::tag_display(&tag.keyword, &tag.name, &stored);
        rows.push(SidebarRow {
            key: format!("pinned-tag:{}", tag.id).into(),
            mailbox: MailboxRow {
                account_id: -2,
                scope: Scope::TabTag(tag.id).to_string().into(),
                context: "Company".into(),
                detail: "folder-pin".into(),
                ..Default::default()
            },
            label: MailLabelRow {
                id,
                account_id: -2,
                account_name: "Company".into(),
                name: tag.name.clone().into(),
                display_name: tag.name.clone().into(),
                color: label_swatch(&color),
                icon_kind,
                ..Default::default()
            },
            ..row(Kind::AccountLabel, "")
        });
    }
    rows
}

/// Company layout: one inbox, tab-wide special mailboxes, one folder tree,
/// one tag list, then the per-account rows under a collapsed Mailboxes section.
#[allow(clippy::too_many_arguments)]
fn make_company_sidebar_rows(
    mailboxes: Vec<MailboxRow>,
    mut unified: Vec<MailboxRow>,
    labels: Vec<MailLabelRow>,
    source_labels: &[flectar_mail_core::models::Label],
    gmail_account_ids: &HashSet<i32>,
    collapsed: &HashSet<String>,
    collapsed_folders: &HashSet<i64>,
    company_ids: &[i64],
    folder_unreads: &HashMap<i64, i64>,
    inbox_count: usize,
    heading_label: &str,
    folder_color: Option<slint::Color>,
    pinned_tags: &[String],
) -> (Vec<SidebarRow>, Vec<MailboxEntry>) {
    use SidebarRowKind as Kind;
    use flectar_mail_core::company_view::{FolderLeaf, merge_folders, merge_tags};

    let company = company_ids.iter().copied().collect::<HashSet<_>>();
    let mut heading = row(Kind::UnifiedHeading, "company-heading");
    heading.mailbox.label = heading_label.into();
    let mut inbox = row(Kind::UnifiedInbox, "company-inbox");
    inbox.mailbox = MailboxRow {
        folder_id: -1,
        is_selectable: true,
        label: "Company Inbox".into(),
        scope: Scope::CompanyInbox.to_string().into(),
        count: if inbox_count > 0 {
            inbox_count.to_string().into()
        } else {
            Default::default()
        },
        ..Default::default()
    };
    unified.sort_by_key(|mailbox| company_unified_rank(mailbox.scope.as_str()));
    let mut rows = vec![heading, inbox];
    rows.extend(unified.into_iter().map(|mailbox| SidebarRow {
        key: format!("unified:{}", mailbox.scope).into(),
        mailbox,
        ..row(Kind::UnifiedMailbox, "")
    }));
    let tag_copies = company_tag_copies(source_labels, &company);
    let merged_tags = merge_tags(&tag_copies);
    rows.extend(pinned_tag_rows(&merged_tags, pinned_tags));
    let leaves = mailboxes
        .iter()
        .filter(|mailbox| {
            !mailbox.is_account
                && !mailbox.is_standard
                && company.contains(&i64::from(mailbox.account_id))
                && !mailbox.detail.is_empty()
        })
        .map(|mailbox| FolderLeaf {
            account_id: i64::from(mailbox.account_id),
            folder_id: i64::from(mailbox.folder_id),
            path: mailbox.detail.to_string(),
            unread: folder_unreads
                .get(&i64::from(mailbox.folder_id))
                .copied()
                .unwrap_or(0),
        })
        .collect::<Vec<_>>();
    let merged = merge_folders(&leaves);
    let mut company_mailboxes = Vec::with_capacity(merged.len());
    let mut hidden_depth = None;
    for node in &merged {
        if let Some(depth) = hidden_depth {
            if node.depth > depth {
                continue;
            }
            hidden_depth = None;
        }
        let expanded = !collapsed_folders.contains(&node.id);
        if node.has_children && !expanded {
            hidden_depth = Some(node.depth);
        }
        let scope = Scope::MergedFolder(node.id);
        company_mailboxes.push(MailboxEntry {
            account_id: -2,
            folder_id: node.id,
            parent_folder_id: node.parent_id,
            depth: usize::try_from(node.depth).unwrap_or(0),
            has_children: node.has_children,
            is_standard: false,
            is_selectable: true,
            can_create_children: true,
            can_rename: true,
            can_delete: true,
            label: node.label.clone(),
            scope: Some(scope),
            context: "Company".to_owned(),
            detail: node.path.clone(),
            avatar: String::new(),
            is_account: false,
            count: if node.unread > 0 {
                node.unread.to_string()
            } else {
                String::new()
            },
        });
        let folder_id = i32::try_from(node.id).unwrap_or(-1);
        rows.push(SidebarRow {
            key: format!("merged-folder:{folder_id}").into(),
            mailbox: MailboxRow {
                account_id: -2,
                folder_id,
                parent_folder_id: i32::try_from(node.parent_id).unwrap_or(-1),
                depth: node.depth,
                has_children: node.has_children,
                expanded,
                is_selectable: true,
                can_create_children: true,
                can_rename: true,
                can_delete: true,
                label: node.label.clone().into(),
                scope: scope.to_string().into(),
                detail: node.path.clone().into(),
                count: if node.unread > 0 {
                    node.unread.to_string().into()
                } else {
                    Default::default()
                },
                ..Default::default()
            },
            ..row(Kind::Folder, "")
        });
    }
    rows.push(SidebarRow {
        mailbox: MailboxRow {
            account_id: -2,
            folder_id: -1,
            context: "Company".into(),
            ..Default::default()
        },
        ..row(Kind::NewFolder, "new-folder:company")
    });
    rows.push(section(
        Kind::GlobalLabelsSection,
        "company-tags",
        collapsed,
    ));
    if !collapsed.contains("company-tags") {
        for tag in &merged_tags {
            let Some(member) = tag.members.first() else {
                continue;
            };
            let Some(id) = i32::try_from(member.label_id).ok() else {
                continue;
            };
            let stored = tag
                .members
                .iter()
                .map(|member| member.color.as_str())
                .collect::<Vec<_>>();
            let (color, icon_kind) = flectar_mail_core::company_view::tag_display(
                &tag.keyword,
                &tag.name,
                &stored,
            );
            let pinned = pinned_tags.iter().any(|keyword| {
                flectar_mail_core::company_view::tag_key(keyword)
                    == flectar_mail_core::company_view::tag_key(&tag.keyword)
            });
            rows.push(SidebarRow {
                key: format!("tab-tag:{}", tag.id).into(),
                mailbox: MailboxRow {
                    account_id: -2,
                    scope: Scope::TabTag(tag.id).to_string().into(),
                    context: "Company".into(),
                    detail: if pinned {
                        "tag-pinned".into()
                    } else {
                        Default::default()
                    },
                    ..Default::default()
                },
                label: MailLabelRow {
                    id,
                    account_id: -2,
                    account_name: "Company".into(),
                    name: tag.name.clone().into(),
                    display_name: tag.name.clone().into(),
                    color: label_swatch(&color),
                    icon_kind,
                    ..Default::default()
                },
                ..row(Kind::AccountLabel, "")
            });
        }
        rows.push(SidebarRow {
            mailbox: MailboxRow {
                account_id: -2,
                context: "Company".into(),
                ..Default::default()
            },
            ..row(Kind::NewLabel, "new-label:company")
        });
    }

    rows.push(section(Kind::AccountsHeading, "mailboxes", collapsed));
    if !collapsed.contains("mailboxes") {
        let scoped = mailboxes
            .into_iter()
            .filter(|mailbox| company.contains(&i64::from(mailbox.account_id)))
            .collect::<Vec<_>>();
        rows.extend(account_rows(
            scoped,
            &labels,
            gmail_account_ids,
            collapsed,
        ));
    }
    if let Some(color) = folder_color {
        for row in &mut rows {
            if row.kind == Kind::Folder {
                row.mailbox.color = color;
                row.mailbox.has_custom_color = true;
            }
        }
    }
    (rows, company_mailboxes)
}

fn filter_company_rows(rows: Vec<SidebarRow>, query: &str) -> Vec<SidebarRow> {
    use SidebarRowKind as Kind;
    let query = query.to_lowercase();
    rows.into_iter()
        .filter(|row| {
            matches!(
                row.kind,
                Kind::Folder
                    | Kind::AccountLabel
                    | Kind::UnifiedMailbox
                    | Kind::UnifiedInbox
                    | Kind::Account
                    | Kind::Important
                    | Kind::Other
                    | Kind::Category
                    | Kind::GlobalLabel
            ) && (row.mailbox.label.to_lowercase().contains(&query)
                || row.mailbox.detail.to_lowercase().contains(&query)
                || row.label.name.to_lowercase().contains(&query))
        })
        .collect()
}

fn unified_rows(unified: Vec<MailboxRow>) -> Vec<SidebarRow> {
    use SidebarRowKind as Kind;
    let mut rows = vec![
        row(Kind::UnifiedHeading, "unified-heading"),
        row(Kind::UnifiedInbox, "unified-inbox"),
    ];
    rows.extend(unified.into_iter().map(|mailbox| SidebarRow {
        key: format!("unified:{}", mailbox.scope).into(),
        mailbox,
        ..row(Kind::UnifiedMailbox, "")
    }));
    rows
}

fn shared_navigation_rows(labels: &[MailLabelRow], collapsed: &HashSet<String>) -> Vec<SidebarRow> {
    use SidebarRowKind as Kind;
    let mut rows = vec![section(Kind::CategoriesSection, "categories", collapsed)];
    if !collapsed.contains("categories") {
        rows.push(row(Kind::Important, "important"));
        rows.push(row(Kind::Other, "other"));
        rows.extend(
            labels
                .iter()
                .filter(|label| label.is_auto)
                .map(|label| SidebarRow {
                    label: label.clone(),
                    ..row(Kind::Category, format!("category:{}", label.id))
                }),
        );
    }
    if labels.iter().any(|label| label.is_global) {
        rows.push(section(
            Kind::GlobalLabelsSection,
            "global-labels",
            collapsed,
        ));
        if !collapsed.contains("global-labels") {
            rows.extend(
                labels
                    .iter()
                    .filter(|label| label.is_global)
                    .map(|label| SidebarRow {
                        key: format!("global-label:{}", label.id).into(),
                        label: label.clone(),
                        ..row(Kind::GlobalLabel, "")
                    }),
            );
        }
    }
    rows
}

fn account_rows(
    mailboxes: Vec<MailboxRow>,
    labels: &[MailLabelRow],
    gmail_account_ids: &HashSet<i32>,
    collapsed: &HashSet<String>,
) -> Vec<SidebarRow> {
    use SidebarRowKind as Kind;
    let mut rows = Vec::new();
    // Group once by stable account id, not display text. Preserve account and
    // folder order even when providers return interleaved mailbox entries.
    let mut accounts = Vec::new();
    let mut folders: HashMap<i32, Vec<MailboxRow>> = HashMap::new();
    for mailbox in mailboxes {
        if mailbox.is_account {
            accounts.push(mailbox);
        } else {
            folders.entry(mailbox.account_id).or_default().push(mailbox);
        }
    }
    for account in accounts {
        let account_id = account.account_id;
        let account_name = account.label.clone();
        let can_create_top_level_mailbox = account.can_create_children;
        let key = format!("account:{account_id}");
        let open = !collapsed.contains(&key);
        rows.push(SidebarRow {
            mailbox: account,
            ..section(Kind::Account, &key, collapsed)
        });
        if open {
            rows.extend(
                folders
                    .remove(&account_id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|mailbox| SidebarRow {
                        key: format!("mailbox:{account_id}:{}", mailbox.scope).into(),
                        mailbox,
                        ..row(Kind::Folder, "")
                    }),
            );
            if !gmail_account_ids.contains(&account_id) && can_create_top_level_mailbox {
                rows.push(SidebarRow {
                    mailbox: MailboxRow {
                        account_id,
                        folder_id: -1,
                        ..Default::default()
                    },
                    ..row(Kind::NewFolder, format!("new-folder:{account_id}"))
                });
            }
            let label_key = format!("account-labels:{account_id}");
            if gmail_account_ids.contains(&account_id)
                || labels.iter().any(|label| label.account_id == account_id)
            {
                rows.push(SidebarRow {
                    mailbox: MailboxRow {
                        account_id,
                        ..Default::default()
                    },
                    ..section(Kind::AccountLabelsSection, &label_key, collapsed)
                });
                if !collapsed.contains(&label_key) {
                    rows.extend(account_label_rows(
                        labels,
                        account_id,
                        gmail_account_ids.contains(&account_id),
                        collapsed,
                    ));
                    if gmail_account_ids.contains(&account_id) {
                        rows.push(SidebarRow {
                            mailbox: MailboxRow {
                                account_id,
                                context: account_name.clone(),
                                ..Default::default()
                            },
                            ..row(Kind::NewLabel, format!("new-label:{account_id}"))
                        });
                    }
                }
            }
        }
    }
    rows
}

/// Named accounts replace the raw mailbox list, except when the open business
/// tab's chosen accounts actually span mailboxes. Those rows are Inbox, Sent,
/// and the account folders.
fn hides_connection_rows(named_accounts: bool, tab_shows_mailboxes: bool) -> bool {
    named_accounts && !tab_shows_mailboxes
}

#[cfg(test)]
mod tests {
    use super::*;
    use SidebarRowKind as Kind;
    use slint::private_unstable_api::re_exports::{
        ModelChangeListener, ModelChangeListenerContainer,
    };
    use std::pin::Pin;

    fn assert_rows_equal(actual: &[SidebarRow], expected: &[SidebarRow]) {
        assert_eq!(actual.len(), expected.len());
        for (a, b) in actual.iter().zip(expected) {
            assert!(same_row(a, b), "{a:?} != {b:?}");
        }
    }

    fn accounts(count: i32, folders: i32) -> Vec<MailboxRow> {
        (1..=count)
            .flat_map(|account_id| {
                let account = MailboxRow {
                    account_id,
                    is_account: true,
                    can_create_children: true,
                    label: "Same display name".into(),
                    context: "Same display name".into(),
                    ..Default::default()
                };
                std::iter::once(account).chain((0..folders).map(move |folder| MailboxRow {
                    account_id,
                    folder_id: account_id * folders + folder,
                    scope: format!("Folder:{}", account_id * folders + folder).into(),
                    label: format!("Folder {folder}").into(),
                    context: "Same display name".into(),
                    depth: folder % 3,
                    has_children: folder % 3 == 0,
                    expanded: true,
                    ..Default::default()
                }))
            })
            .collect()
    }

    fn defaults() -> HashSet<String> {
        HashSet::from(["categories".into(), "global-labels".into()])
    }

    #[test]
    fn single_account_folders_precede_shared_navigation_without_duplicates() {
        let labels = vec![
            MailLabelRow {
                id: 11,
                account_id: 1,
                name: "Travel".into(),
                ..Default::default()
            },
            MailLabelRow {
                id: 12,
                is_global: true,
                name: "Projects".into(),
                ..Default::default()
            },
            MailLabelRow {
                id: 13,
                is_auto: true,
                name: "Newsletters".into(),
                ..Default::default()
            },
        ];
        for count in 0..=2 {
            let rows = make_sidebar_rows(
                SidebarLayout::for_account_count(count as usize),
                accounts(count, 7),
                vec![MailboxRow {
                    scope: "Unified Starred".into(),
                    ..Default::default()
                }],
                labels.clone(),
                &HashSet::new(),
                &HashSet::new(),
                None,
            );
            for kind in [
                Kind::UnifiedHeading,
                Kind::UnifiedInbox,
                Kind::UnifiedMailbox,
                Kind::AccountsHeading,
            ] {
                assert_eq!(
                    rows.iter().filter(|row| row.kind == kind).count(),
                    usize::from(count > 1)
                );
            }
            for kind in [
                Kind::Category,
                Kind::GlobalLabel,
                Kind::Important,
                Kind::Other,
            ] {
                assert_eq!(rows.iter().filter(|row| row.kind == kind).count(), 1);
            }
            assert_eq!(
                rows.iter().filter(|row| row.kind == Kind::Folder).count(),
                (count * 7) as usize
            );
            if count == 1 {
                assert_eq!(rows[0].kind, Kind::Account);
                let last_account_row = rows
                    .iter()
                    .rposition(|row| row.kind == Kind::AccountLabel)
                    .unwrap();
                let shared = rows
                    .iter()
                    .position(|row| row.kind == Kind::CategoriesSection)
                    .unwrap();
                assert!(last_account_row < shared);
            }
        }
    }

    #[test]
    fn changing_account_count_preserves_collapsed_sections_and_row_identity() {
        let collapsed = HashSet::from(["account:1".into(), "categories".into()]);
        let model = SidebarModel::default();
        for count in [2, 1, 2, 0, 1] {
            let rows = make_sidebar_rows(
                SidebarLayout::for_account_count(count as usize),
                accounts(count, 7),
                vec![],
                vec![],
                &HashSet::new(),
                &collapsed,
                None,
            );
            model.reconcile(rows.clone());
            assert_rows_equal(&model.iter().collect::<Vec<_>>(), &rows);
            assert!(
                !rows
                    .iter()
                    .any(|row| row.kind == Kind::Folder && row.mailbox.account_id == 1)
            );
            assert!(!rows.iter().any(|row| row.kind == Kind::Important));
            if count > 0 {
                let account = rows.iter().find(|row| row.key == "account:1").unwrap();
                assert!(!account.open);
            }
        }
    }

    #[test]
    fn new_account_labels_start_collapsed_without_collapsing_accounts() {
        let mut initialized = HashSet::new();
        let mut collapsed = defaults();
        collapse_new_account_label_sections([1, 2], &mut initialized, &mut collapsed, false);
        assert!(!collapsed.contains("account:1"));
        assert!(collapsed.contains("account-labels:1"));
        assert!(!collapsed.contains("account:2"));
        assert!(collapsed.contains("account-labels:2"));

        collapsed.remove("account-labels:1");
        collapse_new_account_label_sections([1, 2, 3], &mut initialized, &mut collapsed, false);
        assert!(!collapsed.contains("account:1"));
        assert!(!collapsed.contains("account-labels:1"));
        assert!(!collapsed.contains("account:3"));
        assert!(collapsed.contains("account-labels:3"));
    }

    #[test]
    fn new_folder_branches_start_collapsed_without_overriding_user_expansion() {
        let mut initialized = HashSet::new();
        let mut collapsed = HashSet::new();
        collapse_new_folder_sections([10, 20], &mut initialized, &mut collapsed);
        assert_eq!(collapsed, HashSet::from([10, 20]));

        collapsed.remove(&10);
        collapse_new_folder_sections([10, 20, 30], &mut initialized, &mut collapsed);
        assert_eq!(collapsed, HashSet::from([20, 30]));
    }

    #[test]
    fn every_folder_occurs_once_and_stays_with_its_account() {
        for (account_count, folder_count) in [(1, 750), (5, 150), (50, 150)] {
            let rows = make_sidebar_rows(
                SidebarLayout::for_account_count(account_count as usize),
                accounts(account_count, folder_count),
                vec![],
                vec![],
                &HashSet::new(),
                &defaults(),
                None,
            );
            assert_eq!(
                rows.iter().filter(|row| row.kind == Kind::Folder).count(),
                (account_count * folder_count) as usize
            );
            assert_eq!(
                rows.iter()
                    .map(|row| &row.key)
                    .collect::<HashSet<_>>()
                    .len(),
                rows.len()
            );
            let mut account = 0;
            for entry in rows {
                match entry.kind {
                    Kind::Account => account = entry.mailbox.account_id,
                    Kind::Folder | Kind::NewFolder => assert_eq!(entry.mailbox.account_id, account),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn account_order_and_interleaved_folders_use_ids_not_display_names() {
        let data = accounts(2, 2);
        let interleaved = [3, 1, 4, 0, 5, 2].map(|i| data[i].clone()).to_vec();
        let rows = make_sidebar_rows(
            SidebarLayout::Unified,
            interleaved,
            vec![],
            vec![],
            &HashSet::new(),
            &defaults(),
            None,
        );
        let ids: Vec<_> = rows
            .iter()
            .filter(|row| row.kind == Kind::Account || row.kind == Kind::Folder)
            .map(|row| (row.kind, row.mailbox.account_id, row.mailbox.folder_id))
            .collect();
        assert_eq!(
            ids,
            vec![
                (Kind::Account, 2, 0),
                (Kind::Folder, 2, 4),
                (Kind::Folder, 2, 5),
                (Kind::Account, 1, 0),
                (Kind::Folder, 1, 2),
                (Kind::Folder, 1, 3)
            ]
        );
    }

    #[test]
    fn same_named_labels_remain_distinct_under_their_accounts() {
        let labels = vec![
            MailLabelRow {
                id: 11,
                account_id: 1,
                name: "Travel".into(),
                ..Default::default()
            },
            MailLabelRow {
                id: 22,
                account_id: 2,
                name: "Travel".into(),
                ..Default::default()
            },
        ];
        let rows = make_sidebar_rows(
            SidebarLayout::Unified,
            accounts(2, 1),
            vec![],
            labels,
            &HashSet::new(),
            &HashSet::new(),
            None,
        );
        let account_labels = rows
            .iter()
            .filter(|row| row.kind == Kind::AccountLabel)
            .map(|row| (row.label.account_id, row.label.id, row.label.name.clone()))
            .collect::<Vec<_>>();

        assert_eq!(account_labels.len(), 2);
        assert_eq!(account_labels[0].0, 1);
        assert_eq!(account_labels[0].1, 11);
        assert_eq!(account_labels[0].2, "Travel");
        assert_eq!(account_labels[1].0, 2);
        assert_eq!(account_labels[1].1, 22);
        assert_eq!(account_labels[1].2, "Travel");
    }

    #[test]
    fn gmail_creates_labels_while_other_accounts_create_folders() {
        let rows = make_sidebar_rows(
            SidebarLayout::Unified,
            accounts(2, 0),
            vec![],
            vec![],
            &HashSet::from([1]),
            &HashSet::new(),
            None,
        );
        assert!(
            rows.iter()
                .any(|row| { row.kind == Kind::NewLabel && row.mailbox.account_id == 1 })
        );
        assert!(
            !rows
                .iter()
                .any(|row| { row.kind == Kind::NewFolder && row.mailbox.account_id == 1 })
        );
        assert!(
            rows.iter()
                .any(|row| { row.kind == Kind::NewFolder && row.mailbox.account_id == 2 })
        );
        assert!(
            !rows
                .iter()
                .any(|row| { row.kind == Kind::NewLabel && row.mailbox.account_id == 2 })
        );
    }

    #[test]
    fn company_sidebar_offers_folders_and_tags_without_categories() {
        let mailboxes = vec![
            MailboxRow {
                account_id: 1,
                is_account: true,
                can_create_children: false,
                label: "Hub".into(),
                ..Default::default()
            },
            MailboxRow {
                account_id: 1,
                folder_id: 10,
                is_selectable: true,
                label: "Clients".into(),
                detail: "Clients".into(),
                ..Default::default()
            },
        ];
        let (rows, entries) = make_company_sidebar_rows(
            mailboxes,
            vec![],
            vec![],
            &[],
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            &[1],
            &HashMap::new(),
            0,
            "Company",
            None,
            &[],
        );
        assert!(
            rows.iter()
                .any(|row| row.kind == Kind::NewFolder && row.mailbox.account_id == -2)
        );
        assert!(rows.iter().any(|row| {
            row.kind == Kind::Folder
                && row.mailbox.account_id == -2
                && row.mailbox.can_create_children
                && row.mailbox.can_rename
                && row.mailbox.can_delete
        }));
        assert!(
            !rows.iter().any(|row| {
                matches!(
                    row.kind,
                    Kind::CategoriesSection | Kind::Important | Kind::Other | Kind::Category
                )
            })
        );
        assert!(rows.iter().any(|row| row.key.as_str() == "company-tags"));
        assert!(
            rows.iter()
                .any(|row| row.kind == Kind::NewLabel && row.mailbox.account_id == -2)
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.can_rename && entry.detail == "Clients")
        );
    }

    #[test]
    fn company_sidebar_tints_folders_and_hides_other_tabs_mailboxes() {
        let tint = slint::Color::from_rgb_u8(0x11, 0x22, 0x33);
        let mailboxes = vec![
            MailboxRow {
                account_id: 1,
                is_account: true,
                label: "Here".into(),
                ..Default::default()
            },
            MailboxRow {
                account_id: 1,
                folder_id: 10,
                is_selectable: true,
                label: "Clients".into(),
                detail: "Clients".into(),
                ..Default::default()
            },
            MailboxRow {
                account_id: 2,
                is_account: true,
                label: "Elsewhere".into(),
                ..Default::default()
            },
            MailboxRow {
                account_id: 2,
                folder_id: 20,
                is_selectable: true,
                label: "Private".into(),
                detail: "Private".into(),
                ..Default::default()
            },
        ];
        let (rows, _) = make_company_sidebar_rows(
            mailboxes,
            vec![],
            vec![],
            &[],
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            &[1],
            &HashMap::new(),
            0,
            "TM",
            Some(tint),
            &[],
        );
        assert_eq!(
            rows.iter()
                .find(|row| row.key.as_str() == "company-heading")
                .unwrap()
                .mailbox
                .label
                .as_str(),
            "TM"
        );
        assert!(rows.iter().any(|row| {
            row.kind == Kind::Folder
                && row.mailbox.label.as_str() == "Clients"
                && row.mailbox.has_custom_color
                && row.mailbox.color == tint
        }));
        assert!(
            rows.iter()
                .filter(|row| row.kind == Kind::Folder)
                .all(|row| row.mailbox.has_custom_color && row.mailbox.color == tint)
        );
        assert!(
            !rows
                .iter()
                .any(|row| row.mailbox.label.as_str() == "Elsewhere"
                    || row.mailbox.label.as_str() == "Private")
        );
    }

    #[test]
    fn named_accounts_follow_company_inbox_on_a_business_tab() {
        let company = vec![
            row(Kind::UnifiedHeading, "company-heading"),
            row(Kind::UnifiedInbox, "company-inbox"),
            row(Kind::UnifiedMailbox, "unified:Sent"),
        ];
        let named = vec![row(Kind::AddressInbox, "inbox-account:-7")];
        let rows = place_named_account_rows(true, company, named);
        assert_eq!(
            rows.iter()
                .map(|row| row.key.to_string())
                .collect::<Vec<_>>(),
            vec![
                "company-heading",
                "company-inbox",
                "inbox-account:-7",
                "unified:Sent",
            ]
        );
        let plain = place_named_account_rows(
            false,
            vec![row(Kind::Account, "account:1")],
            vec![row(Kind::AddressInbox, "inbox-account:-7")],
        );
        assert_eq!(plain[0].key.as_str(), "inbox-account:-7");
    }

    #[test]
    fn local_mail_follows_what_a_single_account_shares() {
        // One account, then local mail with one folder.
        let rows = make_sidebar_rows(
            SidebarLayout::SingleAccount,
            accounts(2, 1),
            vec![],
            vec![],
            &HashSet::new(),
            &defaults(),
            Some(2),
        );
        let categories = rows
            .iter()
            .position(|row| row.kind == Kind::CategoriesSection)
            .unwrap();
        assert!(
            rows[..categories]
                .iter()
                .all(|row| row.mailbox.account_id != 2)
        );
        assert_eq!(rows[categories + 1].kind, Kind::Account);
        assert!(
            rows[categories + 1..]
                .iter()
                .all(|row| row.mailbox.account_id == 2)
        );

        // With several accounts it comes last already: the mailbox list puts
        // it after them.
        let rows = make_sidebar_rows(
            SidebarLayout::Unified,
            accounts(3, 1),
            vec![],
            vec![],
            &HashSet::new(),
            &defaults(),
            Some(3),
        );
        let local = rows
            .iter()
            .position(|row| row.mailbox.account_id == 3)
            .unwrap();
        assert!(
            rows[..local]
                .iter()
                .any(|row| row.kind == Kind::AccountsHeading)
        );
        assert!(rows[local..].iter().all(|row| row.mailbox.account_id == 3));
    }

    #[test]
    fn account_without_root_creation_right_has_no_new_folder_action() {
        let mut rows_input = accounts(1, 0);
        rows_input[0].can_create_children = false;
        let rows = make_sidebar_rows(
            SidebarLayout::SingleAccount,
            rows_input,
            vec![],
            vec![],
            &HashSet::new(),
            &HashSet::new(),
            None,
        );
        assert!(!rows.iter().any(|row| row.kind == Kind::NewFolder));
    }

    #[test]
    fn gmail_labels_form_a_collapsible_tree_without_inventing_parents() {
        let label = |id, name: &str| MailLabelRow {
            id,
            account_id: 1,
            name: name.into(),
            display_name: name.into(),
            ..Default::default()
        };
        let labels = vec![
            label(1, "Projects"),
            label(2, "Projects/Launch"),
            label(3, "Projects/Launch/Design"),
            label(4, "Orphan/Child"),
        ];
        let rows = make_sidebar_rows(
            SidebarLayout::SingleAccount,
            accounts(1, 0),
            vec![],
            labels.clone(),
            &HashSet::from([1]),
            &HashSet::new(),
            None,
        );
        let visible = rows
            .iter()
            .filter(|row| row.kind == Kind::AccountLabel)
            .map(|row| {
                (
                    row.label.id,
                    row.label.display_name.to_string(),
                    row.label.depth,
                    row.label.has_children,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            visible,
            [
                (1, "Projects".into(), 0, true),
                (2, "Launch".into(), 1, true),
                (3, "Design".into(), 2, false),
                (4, "Orphan/Child".into(), 0, false),
            ]
        );

        let collapsed = make_sidebar_rows(
            SidebarLayout::SingleAccount,
            accounts(1, 0),
            vec![],
            labels,
            &HashSet::from([1]),
            &HashSet::from(["gmail-label:1:1".into()]),
            None,
        );
        assert_eq!(
            collapsed
                .iter()
                .filter(|row| row.kind == Kind::AccountLabel)
                .map(|row| row.label.id)
                .collect::<Vec<_>>(),
            [1, 4]
        );
        assert!(
            !collapsed
                .iter()
                .find(|row| row.label.id == 1)
                .unwrap()
                .label
                .expanded
        );
    }

    #[test]
    fn sidebar_filter_finds_account_and_global_labels() {
        let labels = vec![
            MailLabelRow {
                id: 11,
                account_id: 1,
                name: "Travel".into(),
                ..Default::default()
            },
            MailLabelRow {
                id: 22,
                name: "Travel plans".into(),
                is_global: true,
                ..Default::default()
            },
        ];
        let rows = make_filtered_sidebar_rows(accounts(2, 0), labels, "travel", &HashSet::new());
        assert!(
            rows.iter()
                .any(|row| row.kind == Kind::AccountLabel && row.label.id == 11)
        );
        assert!(
            rows.iter()
                .any(|row| row.kind == Kind::GlobalLabel && row.label.id == 22)
        );
    }

    #[test]
    fn section_state_survives_parent_collapse_refresh_and_reorder() {
        let labels = vec![
            MailLabelRow {
                id: 1,
                is_auto: true,
                ..Default::default()
            },
            MailLabelRow {
                id: 2,
                is_global: true,
                ..Default::default()
            },
        ];
        let mut collapsed = HashSet::from(["account:1".into()]);
        let open = make_sidebar_rows(
            SidebarLayout::Unified,
            accounts(2, 3),
            vec![],
            labels.clone(),
            &HashSet::new(),
            &collapsed,
            None,
        );
        assert!(open.iter().any(|row| row.kind == Kind::Category));
        assert!(open.iter().any(|row| row.kind == Kind::GlobalLabel));
        assert!(
            !open.iter().any(|row| row.mailbox.account_id == 1
                && matches!(row.kind, Kind::Folder | Kind::NewFolder))
        );
        collapsed.insert("categories".into());
        collapsed.insert("global-labels".into());
        let closed = make_sidebar_rows(
            SidebarLayout::Unified,
            accounts(2, 3),
            vec![],
            labels.clone(),
            &HashSet::new(),
            &collapsed,
            None,
        );
        assert!(
            !closed
                .iter()
                .any(|row| matches!(row.kind, Kind::Category | Kind::GlobalLabel))
        );
        collapsed.remove("categories");
        collapsed.remove("global-labels");
        assert_rows_equal(
            &open,
            &make_sidebar_rows(
                SidebarLayout::Unified,
                accounts(2, 3),
                vec![],
                labels,
                &HashSet::new(),
                &collapsed,
                None,
            ),
        );
        let mut reordered = accounts(2, 3);
        reordered.rotate_left(4);
        let rows = make_sidebar_rows(
            SidebarLayout::Unified,
            reordered,
            vec![],
            vec![],
            &HashSet::new(),
            &collapsed,
            None,
        );
        assert!(!rows.iter().find(|row| row.key == "account:1").unwrap().open);
        assert!(rows.iter().find(|row| row.key == "account:2").unwrap().open);
    }

    #[test]
    fn folder_filter_is_case_insensitive_and_keeps_account_context() {
        let mut data = accounts(2, 3);
        data[2].label = "Receipts".into();
        data[5].label = "RECEIPTS 2025".into();

        let rows = make_filtered_sidebar_rows(data, vec![], "receipts", &HashSet::new());
        assert_eq!(
            rows.iter().map(|row| row.kind).collect::<Vec<_>>(),
            vec![Kind::Account, Kind::Folder, Kind::Account, Kind::Folder]
        );
        assert!(
            rows.iter()
                .filter(|row| row.kind == Kind::Account)
                .all(|row| row.open)
        );
        assert!(
            rows.iter()
                .filter(|row| row.kind == Kind::Folder)
                .all(|row| { row.mailbox.depth == 0 && !row.mailbox.has_children })
        );
    }

    #[derive(Debug, PartialEq)]
    enum Event {
        Added(usize, usize),
        Removed(usize, usize),
        Changed(usize),
    }
    struct Observer {
        model: Rc<SidebarModel>,
        events: Rc<RefCell<Vec<Event>>>,
    }
    impl Observer {
        fn record(&self, event: Event) {
            // Notifications must arrive after mutation with no outstanding borrow.
            for i in 0..self.model.row_count() {
                assert!(self.model.row_data(i).is_some());
            }
            self.events.borrow_mut().push(event);
        }
    }
    impl ModelChangeListener for Observer {
        fn row_changed(self: Pin<&Self>, row: usize) {
            self.record(Event::Changed(row));
        }
        fn row_added(self: Pin<&Self>, index: usize, count: usize) {
            self.record(Event::Added(index, count));
        }
        fn row_removed(self: Pin<&Self>, index: usize, count: usize) {
            self.record(Event::Removed(index, count));
        }
        fn reset(self: Pin<&Self>) {
            panic!("sidebar must not reset its model");
        }
    }

    #[test]
    fn refresh_is_silent_and_large_expansion_uses_one_range_notification() {
        let model = Rc::new(SidebarModel::default());
        let events = Rc::new(RefCell::new(Vec::new()));
        let observer = Box::pin(ModelChangeListenerContainer::new(Observer {
            model: model.clone(),
            events: events.clone(),
        }));
        model
            .model_tracker()
            .attach_peer(observer.as_ref().model_peer());
        let closed = vec![
            section(
                Kind::Account,
                "account:1",
                &HashSet::from(["account:1".into()]),
            ),
            row(Kind::Folder, "tail"),
        ];
        model.reconcile(closed.clone());
        events.borrow_mut().clear();
        model.reconcile(closed.clone());
        assert!(events.borrow().is_empty());
        let mut expanded = vec![section(Kind::Account, "account:1", &HashSet::new())];
        expanded.extend((0..10_000).map(|id| row(Kind::Folder, format!("folder:{id}"))));
        expanded.push(row(Kind::Folder, "tail"));
        model.reconcile(expanded.clone());
        assert_eq!(
            *events.borrow(),
            vec![Event::Added(1, 10_000), Event::Changed(0)]
        );
        events.borrow_mut().clear();
        expanded[500].mailbox.count = "99".into();
        model.reconcile(expanded.clone());
        assert_eq!(*events.borrow(), vec![Event::Changed(500)]);
        events.borrow_mut().clear();
        model.reconcile(closed.clone());
        assert_eq!(
            *events.borrow(),
            vec![Event::Removed(1, 10_000), Event::Changed(0)]
        );
        assert_rows_equal(&model.iter().collect::<Vec<_>>(), &closed);
    }

    #[test]
    fn pinned_tags_sit_under_starred_and_select_the_same_tag() {
        use flectar_mail_core::models::Label;
        let label = Label {
            id: 7,
            name: "Needs Action".into(),
            color: "#6b7280".into(),
            keyword: "$label:needs-action".into(),
            position: 0,
            owner_account_id: Some(1),
            is_auto: false,
        };
        let unified = vec![MailboxRow {
            label: "Starred".into(),
            scope: "Unified Starred".into(),
            ..Default::default()
        }];
        let (rows, _) = make_company_sidebar_rows(
            Vec::new(),
            unified,
            Vec::new(),
            &[label],
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            &[1],
            &HashMap::new(),
            0,
            "Company",
            None,
            &["needs-action".into()],
        );
        let keys = rows
            .iter()
            .map(|row| row.key.to_string())
            .collect::<Vec<_>>();
        let starred = keys.iter().position(|key| key.contains("Starred")).unwrap();
        let pinned = keys
            .iter()
            .position(|key| key.starts_with("pinned-tag:"))
            .unwrap();
        let tags = keys.iter().position(|key| key == "company-tags").unwrap();
        assert!(starred < pinned && pinned < tags);
        let pin = &rows[pinned];
        let tag = rows
            .iter()
            .find(|row| row.key.starts_with("tab-tag:"))
            .unwrap();
        assert_eq!(pin.mailbox.scope, tag.mailbox.scope);
        assert_eq!(pin.mailbox.detail.as_str(), "folder-pin");
        assert_eq!(tag.mailbox.detail.as_str(), "tag-pinned");
        assert_eq!(pin.label.name.as_str(), "Needs Action");
    }

    #[test]
    fn reconciliation_handles_empty_replacement_and_reordering() {
        let model = SidebarModel::default();
        for keys in [
            vec![],
            vec!["a", "b", "c"],
            vec!["c", "a", "b"],
            vec!["x", "y"],
            vec![],
            vec!["tail"],
        ] {
            let rows: Vec<_> = keys.into_iter().map(|key| row(Kind::Folder, key)).collect();
            model.reconcile(rows.clone());
            assert_rows_equal(&model.iter().collect::<Vec<_>>(), &rows);
        }
    }

    #[test]
    fn accordion_transition_keeps_closing_rows_in_order_and_reveals_new_rows() {
        let account = section(Kind::Account, "account:1", &HashSet::new());
        let mut closed_account = account.clone();
        closed_account.open = false;
        let category = section(Kind::CategoriesSection, "categories", &HashSet::new());
        let mut closed_category = category.clone();
        closed_category.open = false;
        let open = vec![
            category.clone(),
            row(Kind::Category, "category:1"),
            account.clone(),
            row(Kind::Folder, "folder:1"),
            row(Kind::Folder, "folder:2"),
            row(Kind::Account, "account:2"),
        ];
        let closed = vec![
            closed_category,
            closed_account,
            row(Kind::Account, "account:2"),
        ];
        assert!(accordion_changed(&open, &closed));
        let closing = transition_rows(open.clone(), closed.clone(), true);
        assert_eq!(
            closing
                .iter()
                .map(|row| row.key.as_str())
                .collect::<Vec<_>>(),
            [
                "categories",
                "category:1",
                "account:1",
                "folder:1",
                "folder:2",
                "account:2"
            ]
        );
        assert!(
            closing
                .iter()
                .filter(|row| !row.show_row)
                .all(|row| { matches!(row.key.as_str(), "category:1" | "folder:1" | "folder:2") })
        );
        assert_eq!(closing.iter().filter(|row| !row.show_row).count(), 3);

        let reopening = transition_rows(closing, open, true);
        assert!(reopening.iter().all(|row| row.show_row && !row.reveal_row));
        let fresh = transition_rows(closed, reopening, true);
        assert!(
            fresh
                .iter()
                .filter(|row| row.reveal_row)
                .all(|row| { matches!(row.key.as_str(), "category:1" | "folder:1" | "folder:2") })
        );
        assert_eq!(fresh.iter().filter(|row| row.reveal_row).count(), 3);

        let mut folder = row(Kind::Folder, "folder:tree");
        folder.mailbox.expanded = true;
        let mut collapsed_folder = folder.clone();
        collapsed_folder.mailbox.expanded = false;
        assert!(accordion_changed(&[folder], &[collapsed_folder]));

        let mut label = row(Kind::AccountLabel, "gmail-label:1:7");
        label.label.expanded = true;
        let mut collapsed_label = label.clone();
        collapsed_label.label.expanded = false;
        assert!(accordion_changed(&[label], &[collapsed_label]));
    }

    #[test]
    fn large_accordion_uses_batched_model_reconciliation() {
        let model = Rc::new(SidebarModel::default());
        let animation = Rc::new(SidebarAnimation::default());
        let mut open = vec![section(Kind::Account, "account:1", &HashSet::new())];
        open.extend((0..1_000).map(|id| row(Kind::Folder, format!("folder:{id}"))));
        animation.reconcile(&model, open, false);
        let closed = vec![section(
            Kind::Account,
            "account:1",
            &HashSet::from(["account:1".into()]),
        )];
        animation.reconcile(&model, closed.clone(), true);
        assert_eq!(model.row_count(), 1);
        assert_rows_equal(&model.iter().collect::<Vec<_>>(), &closed);
    }

    #[test]
    fn large_same_size_replacement_does_not_retain_old_rows() {
        let model = Rc::new(SidebarModel::default());
        let animation = Rc::new(SidebarAnimation::default());
        let mut previous = vec![section(Kind::Account, "account:1", &HashSet::new())];
        previous.extend((0..300).map(|id| row(Kind::Folder, format!("old:{id}"))));
        animation.reconcile(&model, previous, false);
        let mut replacement = vec![section(
            Kind::Account,
            "account:1",
            &HashSet::from(["account:1".into()]),
        )];
        replacement.extend((0..300).map(|id| row(Kind::Folder, format!("new:{id}"))));
        animation.reconcile(&model, replacement.clone(), true);
        assert_rows_equal(&model.iter().collect::<Vec<_>>(), &replacement);
    }

    #[test]
    fn refresh_cancels_closing_rows_and_rapid_reopen_restores_them() {
        let model = Rc::new(SidebarModel::default());
        let animation = Rc::new(SidebarAnimation::default());
        let open = vec![
            section(Kind::CategoriesSection, "categories", &HashSet::new()),
            row(Kind::Important, "important"),
            row(Kind::Other, "other"),
            row(Kind::AccountsHeading, "accounts-heading"),
        ];
        let closed = vec![
            section(
                Kind::CategoriesSection,
                "categories",
                &HashSet::from(["categories".into()]),
            ),
            row(Kind::AccountsHeading, "accounts-heading"),
        ];
        animation.reconcile(&model, open.clone(), false);
        animation.reconcile(&model, closed.clone(), true);
        assert_eq!(model.iter().filter(|row| !row.show_row).count(), 2);
        animation.reconcile(&model, open.clone(), true);
        assert_rows_equal(&model.iter().collect::<Vec<_>>(), &open);

        animation.reconcile(&model, closed, true);
        let filtered = vec![row(Kind::Account, "account:1")];
        animation.reconcile(&model, filtered.clone(), false);
        assert_rows_equal(&model.iter().collect::<Vec<_>>(), &filtered);
    }

    #[test]
    fn a_named_account_spanning_mailboxes_keeps_inbox_sent_and_its_folders() {
        use flectar_mail_core::models::InboxAccount;

        let mut presentation = super::super::AccountPresentationSettings {
            inbox_accounts: vec![InboxAccount {
                id: -7,
                name: "Example Media".into(),
                icon: None,
                color: None,
                account_ids: vec![9, 4, 9, 2],
                address_view_ids: vec![],
                automatic_profile: None,
                profile_id: None,
            }],
            business_tabs: serde_json::from_value(serde_json::json!([
                {"id":"tm","name":"TM","profileIds":[]}
            ]))
            .unwrap(),
            active_business_tab: Some("tm".into()),
            ..Default::default()
        };
        presentation
            .business_tab_accounts
            .insert("tm".into(), vec![-7]);

        let (sidebar, query, scope) = presentation.view_scope();
        assert_eq!(sidebar, Some(vec![2, 4, 9]));
        assert_eq!(query, Some(vec![2, 4, 9]));
        assert_eq!(scope.or_account_ids, vec![2, 4, 9]);
        assert_eq!(
            SidebarLayout::for_account_count(sidebar.unwrap().len()),
            SidebarLayout::Unified
        );
        assert!(!hides_connection_rows(true, true));
        assert!(hides_connection_rows(true, false));

        let mailboxes = [2, 4, 9]
            .into_iter()
            .flat_map(|account_id| {
                let header = MailboxRow {
                    account_id,
                    is_account: true,
                    can_create_children: true,
                    label: format!("Mailbox {account_id}").into(),
                    ..Default::default()
                };
                std::iter::once(header).chain(["Inbox", "Sent", "Clients"].into_iter().map(
                    move |name| MailboxRow {
                        account_id,
                        folder_id: account_id,
                        scope: format!("{name}:{account_id}").into(),
                        label: name.into(),
                        ..Default::default()
                    },
                ))
            })
            .collect::<Vec<_>>();
        let unified = vec![MailboxRow {
            scope: "Unified Sent".into(),
            label: "Sent".into(),
            ..Default::default()
        }];
        let shows = |rows: &[SidebarRow], label: &str| {
            rows.iter().any(|row| row.mailbox.label.as_str() == label)
        };
        let expanded = make_sidebar_rows(
            SidebarLayout::Unified,
            mailboxes.clone(),
            unified.clone(),
            vec![],
            &HashSet::new(),
            &HashSet::new(),
            None,
        );
        let unified_inbox = crate::SidebarRowKind::UnifiedInbox;
        let folder = crate::SidebarRowKind::Folder;
        assert!(expanded.iter().any(|row| row.kind == unified_inbox));
        assert!(shows(&expanded, "Sent"));
        assert!(
            expanded
                .iter()
                .any(|row| row.kind == folder && row.mailbox.label.as_str() == "Clients")
        );
        assert!(
            expanded
                .iter()
                .any(|row| row.kind == folder && row.mailbox.label.as_str() == "Inbox")
        );

        let collapsed = make_sidebar_rows(
            SidebarLayout::Unified,
            mailboxes,
            unified,
            vec![],
            &HashSet::new(),
            &HashSet::from(["account:2".into(), "account:4".into(), "account:9".into()]),
            None,
        );
        assert!(collapsed.iter().any(|row| row.kind == unified_inbox));
        assert!(shows(&collapsed, "Sent"));
        assert!(
            collapsed
                .iter()
                .all(|row| row.mailbox.label.as_str() != "Clients")
        );
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
    use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};

    struct Headless(Rc<MinimalSoftwareWindow>, Rc<Cell<Duration>>);
    impl Platform for Headless {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }
        fn duration_since_start(&self) -> Duration {
            self.1.get()
        }
    }

    #[test]
    fn workspace_sidebars_survive_scrolling_and_layout_recreation() {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(Headless(window.clone(), clock.clone()))).unwrap();
        let app = AppWindow::new().unwrap();
        app.set_startup_ready(true);
        app.set_startup_hydrated(true);
        app.set_connected_accounts(ModelRc::new(VecModel::from(vec![
            AccountRow {
                id: 1,
                ..Default::default()
            },
            AccountRow {
                id: 2,
                ..Default::default()
            },
        ])));
        app.window().set_size(slint::PhysicalSize::new(1320, 800));
        let mut mailboxes = vec![MailboxRow {
            account_id: 1,
            label: "Large account".into(),
            is_account: true,
            ..Default::default()
        }];
        mailboxes.extend((0..1_000).map(|id| MailboxRow {
            account_id: 1,
            folder_id: id,
            scope: format!("Folder:{id}").into(),
            label: format!("Folder {id}").into(),
            expanded: true,
            ..Default::default()
        }));
        mailboxes.push(MailboxRow {
            account_id: 2,
            label: "Second account".into(),
            is_account: true,
            ..Default::default()
        });
        let collapsed = Rc::new(RefCell::new(HashSet::from([
            "categories".into(),
            "global-labels".into(),
        ])));
        let model = Rc::new(SidebarModel::default());
        let animation = Rc::new(SidebarAnimation::default());
        animation.reconcile(
            &model,
            make_sidebar_rows(
                SidebarLayout::Unified,
                mailboxes.clone(),
                vec![],
                vec![],
                &HashSet::new(),
                &collapsed.borrow(),
                None,
            ),
            false,
        );
        app.set_sidebar_rows(model.clone().into());
        let model_for_click = model.clone();
        let animation_for_click = animation.clone();
        let collapsed_for_click = collapsed.clone();
        let motion_for_click = app.as_weak();
        app.on_toggle_sidebar_section(move |key, open| {
            if open {
                collapsed_for_click.borrow_mut().remove(key.as_str());
            } else {
                collapsed_for_click.borrow_mut().insert(key.to_string());
            }
            let animate = motion_for_click
                .upgrade()
                .is_some_and(|app| app.global::<MotionSettings>().get_enabled());
            animation_for_click.reconcile(
                &model_for_click,
                make_sidebar_rows(
                    SidebarLayout::Unified,
                    mailboxes.clone(),
                    vec![],
                    vec![],
                    &HashSet::new(),
                    &collapsed_for_click.borrow(),
                    None,
                ),
                animate,
            );
        });
        app.show().unwrap();
        let draw_after = |elapsed: Duration| {
            clock.set(clock.get() + elapsed);
            slint::platform::update_timers_and_animations();
            window.request_redraw();
            let mut frame = Vec::new();
            window.draw_if_needed(|renderer| {
                let size = window.size();
                let mut pixels =
                    vec![slint::Rgb8Pixel::default(); (size.width * size.height) as usize];
                renderer.render(&mut pixels, size.width as usize);
                if let Some(directory) = std::env::var_os("FLECTAR_SIDEBAR_TEST_SCREENSHOTS") {
                    let directory = PathBuf::from(directory);
                    std::fs::create_dir_all(&directory).unwrap();
                    let bytes: Vec<u8> = pixels.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
                    image::save_buffer(
                        directory.join(format!("frame-{:05}.png", clock.get().as_millis())),
                        &bytes,
                        size.width,
                        size.height,
                        image::ColorType::Rgb8,
                    )
                    .unwrap();
                }
                frame = pixels;
            });
            frame
        };
        let draw = || {
            draw_after(Duration::from_secs(1));
        };
        let dispatch_click = |x, y| {
            let position = slint::LogicalPosition::new(x, y);
            app.window().dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
            app.window().dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
        };
        let click = |x, y| {
            dispatch_click(x, y);
            draw();
        };
        let closed_frame = draw_after(Duration::from_secs(1));
        assert!(model.row_count() > 1_000);
        // Desktop workspaces sit below the 44px top bar; the compact drawer
        // starts at the top of the window.
        const TOP_BAR_HEIGHT: f32 = 44.;
        const COMPACT_CATEGORY_HEADER_CENTER_Y: f32 = 169.;
        const COMPACT_ACCOUNT_HEADER_CENTER_Y: f32 = 232.;
        const CATEGORY_HEADER_CENTER_Y: f32 = COMPACT_CATEGORY_HEADER_CENTER_Y + TOP_BAR_HEIGHT;
        const ACCOUNT_HEADER_CENTER_Y: f32 = COMPACT_ACCOUNT_HEADER_CENTER_Y + TOP_BAR_HEIGHT;
        dispatch_click(140., CATEGORY_HEADER_CENTER_Y);
        assert!(!collapsed.borrow().contains("categories"));
        assert!(
            model
                .iter()
                .any(|row| row.kind == SidebarRowKind::Important && row.reveal_row)
        );
        draw_after(Duration::from_millis(16));
        let opening_frame = draw_after(Duration::from_millis(100));
        let opened_frame = draw_after(Duration::from_millis(250));
        let sidebar_region = |pixels: &[slint::Rgb8Pixel]| {
            pixels
                .chunks(1320)
                .skip(180)
                .take(130)
                .flat_map(|line| line[..250].iter().map(|pixel| (pixel.r, pixel.g, pixel.b)))
                .collect::<Vec<_>>()
        };
        assert_ne!(
            sidebar_region(&opening_frame),
            sidebar_region(&closed_frame)
        );
        assert_ne!(
            sidebar_region(&opening_frame),
            sidebar_region(&opened_frame)
        );
        dispatch_click(140., CATEGORY_HEADER_CENTER_Y);
        assert!(collapsed.borrow().contains("categories"));
        assert!(
            model
                .iter()
                .any(|row| row.kind == SidebarRowKind::Important && !row.show_row)
        );
        let closing_frame = draw_after(Duration::from_millis(100));
        assert_ne!(
            sidebar_region(&closing_frame),
            sidebar_region(&closed_frame)
        );
        assert_ne!(
            sidebar_region(&closing_frame),
            sidebar_region(&opened_frame)
        );
        draw();
        app.global::<MotionSettings>().set_enabled(false);
        dispatch_click(140., CATEGORY_HEADER_CENTER_Y);
        assert!(!collapsed.borrow().contains("categories"));
        assert!(model.iter().all(|row| !row.reveal_row));
        dispatch_click(140., CATEGORY_HEADER_CENTER_Y);
        assert!(collapsed.borrow().contains("categories"));
        assert!(model.iter().all(|row| row.show_row));
        app.global::<MotionSettings>().set_enabled(true);
        draw();
        click(140., ACCOUNT_HEADER_CENTER_Y);
        assert!(collapsed.borrow().contains("account:1"));
        assert!(model.row_count() < 10);
        // Hiding/showing the whole sidebar and switching desktop layouts
        // destroys delegates, but must retain the same account expansion state.
        app.set_mail_sidebar_collapsed(true);
        draw();
        app.set_mail_sidebar_collapsed(false);
        app.set_workspace_layout("minimal".into());
        draw();
        app.set_workspace_layout("default".into());
        draw();
        assert!(collapsed.borrow().contains("account:1"));
        click(140., ACCOUNT_HEADER_CENTER_Y);
        assert!(!collapsed.borrow().contains("account:1"));
        assert!(model.row_count() > 1_000);
        // Virtualized rows are recreated after scrolling in both directions.
        // Variable-height ListView estimates can change while scrolling. Use
        // a delta larger than the whole tree when returning to the top.
        for delta_y in [-10_000., 1_000_000.] {
            app.window().dispatch_event(WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(140., 500.),
                delta_x: 0.,
                delta_y,
            });
            draw();
        }
        click(140., CATEGORY_HEADER_CENTER_Y);
        assert!(!collapsed.borrow().contains("categories"));
        app.window().set_size(slint::PhysicalSize::new(390, 844));
        draw();
        click(31., 29.); // Open the compact folder drawer.
        click(140., COMPACT_CATEGORY_HEADER_CENTER_Y);
        assert!(collapsed.borrow().contains("categories"));
        click(340., 200.); // Close the drawer through its backdrop.
        click(31., 29.);
        click(140., COMPACT_ACCOUNT_HEADER_CENTER_Y);
        assert!(collapsed.borrow().contains("account:1"));
        // Contacts uses the same retained update machinery and shared row controls.
        app.window().set_size(slint::PhysicalSize::new(1320, 800));
        app.set_active_view("contacts".into());
        app.set_connected_accounts(ModelRc::new(VecModel::from(
            (1..=750)
                .map(|id| AccountRow {
                    id,
                    name: format!("Account {id}").into(),
                    email: format!("account{id}@example.com").into(),
                    ..Default::default()
                })
                .collect::<Vec<_>>(),
        )));
        let contacts = Rc::new(RefCell::new(contacts::ContactDirectoryState::new(
            vec![],
            false,
        )));
        app.set_contact_sidebar_rows(contacts.borrow().sidebar_rows.clone().into());
        contacts::apply_contact_rows(&app, &contacts);
        let contacts_for_click = contacts.clone();
        let weak = app.as_weak();
        app.on_toggle_contact_section(move |key, open| {
            if open {
                contacts_for_click
                    .borrow_mut()
                    .collapsed_sections
                    .remove(key.as_str());
            } else {
                contacts_for_click
                    .borrow_mut()
                    .collapsed_sections
                    .insert(key.to_string());
            }
            contacts::refresh_contact_sidebar(&weak.unwrap(), &contacts_for_click);
        });
        let selected_scope = Rc::new(RefCell::new(String::new()));
        let selected_for_click = selected_scope.clone();
        app.on_select_contact_scope(move |scope| {
            *selected_for_click.borrow_mut() = scope.to_string()
        });
        draw();
        const CONTACT_FAVORITES_CENTER_Y: f32 = 182. + TOP_BAR_HEIGHT;
        // The account disclosure follows the unified heading/section, three
        // unified scopes, and the accounts heading. Keep the click centered
        // in the 45 px Account 1 row rather than on the heading above it.
        const CONTACT_ACCOUNT_SECTION_CENTER_Y: f32 = 303. + TOP_BAR_HEIGHT;
        click(140., CONTACT_FAVORITES_CENTER_Y);
        assert_eq!(*selected_scope.borrow(), "Favorites");
        click(140., CONTACT_ACCOUNT_SECTION_CENTER_Y);
        assert!(contacts.borrow().collapsed_sections.contains("account:1"));
        for delta_y in [-10_000., 1_000_000.] {
            app.window().dispatch_event(WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(140., 500.),
                delta_x: 0.,
                delta_y,
            });
            draw();
        }
        app.set_contacts_sidebar_collapsed(true);
        draw();
        app.set_contacts_sidebar_collapsed(false);
        draw();
        assert!(contacts.borrow().collapsed_sections.contains("account:1"));
        click(140., CONTACT_ACCOUNT_SECTION_CENTER_Y);
        assert!(!contacts.borrow().collapsed_sections.contains("account:1"));

        // Calendar source toggles survive recycling and unrelated date changes.
        let today = chrono::NaiveDate::from_ymd_opt(2026, 9, 8).unwrap();
        let mut calendar = calendar::LocalCalendarState::new(today);
        calendar.sources = (1..=750)
            .map(|id| calendar::LocalCalendarSource {
                id,
                account_id: 1,
                provider: "generic",
                name: format!("Calendar {id}"),
                color: String::new(),
                read_only: false,
                enabled: true,
                is_default: id == 1,
                last_synced_at: None,
            })
            .collect();
        let calendar = Rc::new(RefCell::new(calendar));
        app.set_calendar_sources(calendar.borrow().source_rows.clone().into());
        calendar::apply_calendar(&app, &calendar.borrow(), today);
        app.set_active_view("calendar".into());
        let calendar_for_click = calendar.clone();
        let weak = app.as_weak();
        app.on_calendar_set_source_enabled(move |id, enabled| {
            let mut state = calendar_for_click.borrow_mut();
            state
                .sources
                .iter_mut()
                .find(|source| source.id == i64::from(id))
                .unwrap()
                .enabled = enabled;
            calendar::apply_calendar(&weak.unwrap(), &state, today);
        });
        draw();
        click(82., 372. + TOP_BAR_HEIGHT); // The first calendar's visibility toggle.
        assert!(!calendar.borrow().sources[0].enabled);
        for delta_y in [-10_000., 1_000_000.] {
            app.window().dispatch_event(WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(140., 500.),
                delta_x: 0.,
                delta_y,
            });
            draw();
        }
        calendar.borrow_mut().selected_date += chrono::Duration::days(1);
        calendar::apply_calendar(&app, &calendar.borrow(), today);
        app.set_calendar_sidebar_collapsed(true);
        draw();
        app.set_calendar_sidebar_collapsed(false);
        draw();
        app.set_active_view("contacts".into());
        draw();
        app.set_active_view("calendar".into());
        draw();
        assert!(!calendar.borrow().source_rows.row_data(0).unwrap().enabled);
        click(82., 372. + TOP_BAR_HEIGHT); // The first calendar's visibility toggle.
        assert!(calendar.borrow().sources[0].enabled);
        app.hide().unwrap();
    }
}
