-- Tab switches paint a page of local mail first, then refresh sidebar badges.
-- These indexes keep inbox unread COUNTs and folder-unread scans on the
-- unread subset instead of walking every cached message in a large mailbox.
CREATE INDEX IF NOT EXISTS idx_messages_folder_unread
    ON messages(folder_id, account_id, thread_id)
    WHERE is_read = 0 AND is_draft = 0 AND thread_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_folders_role
    ON folders(role)
    WHERE role IS NOT NULL;
