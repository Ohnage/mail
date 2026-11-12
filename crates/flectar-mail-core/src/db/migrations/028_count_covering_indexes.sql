-- Applied after first paint (see ensure_count_covering_indexes). Idempotent.
-- Company-tab cold counts drive from unread threads/messages by account.
CREATE INDEX IF NOT EXISTS idx_threads_account_unread
    ON threads(account_id, id)
    WHERE unread_count > 0;

CREATE INDEX IF NOT EXISTS idx_messages_account_folder_unread
    ON messages(account_id, folder_id, thread_id)
    WHERE is_read = 0 AND is_draft = 0 AND thread_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_message_folders_folder_message
    ON message_folders(folder_id, message_id);
