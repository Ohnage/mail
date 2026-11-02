-- NULL means delivery headers have not been examined; [] means examined with no hints.
ALTER TABLE messages ADD COLUMN delivery_recipients_json TEXT;
