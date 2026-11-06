-- Newest uncached messages, so background body download does not scan the
-- whole store to find the next page.
CREATE INDEX idx_messages_uncached_recent
  ON messages(date DESC, id DESC)
  WHERE body_state = 'none';
