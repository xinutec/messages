//! Every schema change, in order. Each runs once, tracked by its array index in
//! `schema_version`: append new entries, never insert or edit one.

/// Migration v48: backfills v47 from `signal_frames`, so it reaches only as far
/// back as the frames. `JSON_VALUE` because MariaDB rejects `->>`. It matches on
/// the timestamp alone, unlike v52–v54, which also match the sender.
///
/// Public so tests run the statement the migration runs.
pub const BACKFILL_SERVER_TIMES: &str = r"UPDATE messages m
        JOIN signal_frames f ON f.envelope_ts = m.server_ts
         SET m.server_received_ts = COALESCE(
                 m.server_received_ts,
                 JSON_VALUE(f.frame, '$.envelope.serverReceivedTimestamp')),
             m.server_delivered_ts = COALESCE(
                 m.server_delivered_ts,
                 JSON_VALUE(f.frame, '$.envelope.serverDeliveredTimestamp')),
             m.expires_in_seconds = COALESCE(
                 m.expires_in_seconds,
                 JSON_VALUE(f.frame, '$.envelope.dataMessage.expiresInSeconds'),
                 JSON_VALUE(f.frame, '$.envelope.syncMessage.sentMessage.expiresInSeconds'))";

/// Migration v52; public so tests run the statement the migration runs.
pub const BACKFILL_QUOTES: &str = r"UPDATE messages m
        JOIN signal_frames f ON f.envelope_ts = m.server_ts AND f.source_uuid = m.sender_uuid
         SET m.quote_author_uuid = COALESCE(
                 m.quote_author_uuid,
                 JSON_VALUE(f.frame, '$.envelope.dataMessage.quote.authorUuid'),
                 JSON_VALUE(f.frame, '$.envelope.syncMessage.sentMessage.quote.authorUuid')),
             m.quote_text = COALESCE(
                 m.quote_text,
                 NULLIF(JSON_VALUE(f.frame, '$.envelope.dataMessage.quote.text'), ''),
                 NULLIF(JSON_VALUE(f.frame, '$.envelope.syncMessage.sentMessage.quote.text'), ''))";

/// Migration v53; public so tests run the statement the migration runs.
pub const BACKFILL_TEXT_STYLES: &str = r"INSERT IGNORE INTO signal_text_styles (message_id, style, start_utf16, length_utf16)
        SELECT m.id, s.style, s.start_utf16, s.length_utf16
          FROM messages m
          JOIN signal_frames f ON f.envelope_ts = m.server_ts AND f.source_uuid = m.sender_uuid
          JOIN JSON_TABLE(
                   COALESCE(
                       JSON_EXTRACT(f.frame, '$.envelope.dataMessage.textStyles'),
                       JSON_EXTRACT(f.frame, '$.envelope.syncMessage.sentMessage.textStyles'),
                       JSON_EXTRACT(f.frame, '$.envelope.editMessage.dataMessage.textStyles'),
                       JSON_EXTRACT(f.frame, '$.envelope.syncMessage.editMessage.dataMessage.textStyles'),
                       JSON_EXTRACT(f.frame, '$.envelope.syncMessage.sentMessage.editMessage.dataMessage.textStyles')),
                   '$[*]' COLUMNS (
                       style VARCHAR(16) PATH '$.style',
                       start_utf16 INT PATH '$.start',
                       length_utf16 INT PATH '$.length')) s
         WHERE s.style IS NOT NULL AND s.start_utf16 >= 0 AND s.length_utf16 >= 0";

/// Migration v54; public so tests run the statement the migration runs.
pub const BACKFILL_LINK_PREVIEWS: &str = r"INSERT IGNORE INTO signal_link_previews (message_id, position, url, title, description)
        SELECT m.id, p.position - 1, p.url, NULLIF(p.title, ''), NULLIF(p.description, '')
          FROM messages m
          JOIN signal_frames f ON f.envelope_ts = m.server_ts AND f.source_uuid = m.sender_uuid
          JOIN JSON_TABLE(
                   COALESCE(
                       JSON_EXTRACT(f.frame, '$.envelope.dataMessage.previews'),
                       JSON_EXTRACT(f.frame, '$.envelope.syncMessage.sentMessage.previews')),
                   '$[*]' COLUMNS (
                       position FOR ORDINALITY,
                       url TEXT PATH '$.url',
                       title TEXT PATH '$.title',
                       description TEXT PATH '$.description')) p
         WHERE p.url IS NOT NULL AND p.url <> ''";

/// Migration v56; public so tests run the statement the migration runs.
pub const BACKFILL_PREVIEW_IMAGES: &str = r"UPDATE signal_link_previews lp
          JOIN messages m ON m.id = lp.message_id
          JOIN signal_frames f ON f.envelope_ts = m.server_ts AND f.source_uuid = m.sender_uuid
          JOIN JSON_TABLE(
                   COALESCE(
                       JSON_EXTRACT(f.frame, '$.envelope.dataMessage.previews'),
                       JSON_EXTRACT(f.frame, '$.envelope.syncMessage.sentMessage.previews')),
                   '$[*]' COLUMNS (
                       position FOR ORDINALITY,
                       image_id VARCHAR(255) PATH '$.image.id',
                       image_content_type VARCHAR(255) PATH '$.image.contentType')) p
            ON p.position - 1 = lp.position
           SET lp.image_id = p.image_id, lp.image_content_type = p.image_content_type
         WHERE lp.image_id IS NULL AND p.image_id IS NOT NULL";

/// Migration v58; public so tests run the statement the migration runs.
pub const BACKFILL_MENTIONS: &str = r"INSERT IGNORE INTO signal_mentions (message_id, start_utf16, length_utf16, uuid)
        SELECT m.id, x.start_utf16, x.length_utf16, x.uuid
          FROM messages m
          JOIN signal_frames f ON f.envelope_ts = m.server_ts AND f.source_uuid = m.sender_uuid
          JOIN JSON_TABLE(
                   COALESCE(
                       JSON_EXTRACT(f.frame, '$.envelope.dataMessage.mentions'),
                       JSON_EXTRACT(f.frame, '$.envelope.syncMessage.sentMessage.mentions'),
                       JSON_EXTRACT(f.frame, '$.envelope.editMessage.dataMessage.mentions'),
                       JSON_EXTRACT(f.frame, '$.envelope.syncMessage.editMessage.dataMessage.mentions'),
                       JSON_EXTRACT(f.frame, '$.envelope.syncMessage.sentMessage.editMessage.dataMessage.mentions')),
                   '$[*]' COLUMNS (
                       start_utf16 INT PATH '$.start',
                       length_utf16 INT PATH '$.length',
                       uuid VARCHAR(64) PATH '$.uuid')) x
         WHERE x.uuid IS NOT NULL AND x.uuid <> '' AND x.start_utf16 >= 0 AND x.length_utf16 >= 0";

pub(super) const MIGRATIONS: &[&str] = &[
    // v0: people, keyed by ACI UUID (E.164 when there is none).
    r"CREATE TABLE IF NOT EXISTS contacts (
        uuid VARCHAR(64) NOT NULL PRIMARY KEY,
        phone VARCHAR(32) NULL,
        profile_name VARCHAR(255) NULL,
        updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
    )",
    // v1: `thread_id` is `dm:<uuid>` or `group:<id>`, the group id being signal-cli's
    // base64 `groupInfo.groupId`. The JSONL importer keys groups on the export's
    // masterKey instead, so imported and live group threads do not merge.
    r"CREATE TABLE IF NOT EXISTS conversations (
        thread_id VARCHAR(80) NOT NULL PRIMARY KEY,
        type ENUM('dm','group') NOT NULL,
        name VARCHAR(255) NULL,
        updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
    )",
    // v2: a Signal timestamp is unique per sender, so `(sender_uuid, server_ts)`
    // dedupes the live feed against the history import.
    r"CREATE TABLE IF NOT EXISTS messages (
        id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        thread_id VARCHAR(80) NOT NULL,
        sender_uuid VARCHAR(64) NOT NULL,
        server_ts BIGINT NOT NULL,
        body TEXT NULL,
        quote_target_ts BIGINT NULL,
        is_outgoing TINYINT(1) NOT NULL DEFAULT 0,
        created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE KEY uniq_sender_ts (sender_uuid, server_ts),
        INDEX idx_thread_ts (thread_id, server_ts)
    )",
    // v3: attachment metadata; `stored_path` is NULL until the bytes are on the volume.
    r"CREATE TABLE IF NOT EXISTS attachments (
        id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        message_id BIGINT NOT NULL,
        content_type VARCHAR(255) NULL,
        file_name VARCHAR(512) NULL,
        size_bytes BIGINT NULL,
        stored_path VARCHAR(1024) NULL,
        INDEX idx_msg (message_id)
    )",
    // v4: reactions, as add/remove events.
    r"CREATE TABLE IF NOT EXISTS reactions (
        id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        thread_id VARCHAR(80) NOT NULL,
        target_ts BIGINT NOT NULL,
        author_uuid VARCHAR(64) NOT NULL,
        emoji VARCHAR(32) NULL,
        reaction_ts BIGINT NOT NULL,
        removed TINYINT(1) NOT NULL DEFAULT 0,
        UNIQUE KEY uniq_reaction (author_uuid, target_ts, reaction_ts)
    )",
    // v5: delete-for-everyone flags the row; the text is kept.
    r"ALTER TABLE messages
        ADD COLUMN deleted TINYINT(1) NOT NULL DEFAULT 0,
        ADD COLUMN deleted_at TIMESTAMP NULL",
    // v6: an edit is its own row whose `edit_of_ts` points at the original, which is
    // flagged `edited`. The current text is the group's newest row.
    r"ALTER TABLE messages
        ADD COLUMN edited TINYINT(1) NOT NULL DEFAULT 0,
        ADD COLUMN edit_of_ts BIGINT NULL,
        ADD INDEX idx_edit_of (edit_of_ts)",
    // v7: one IRC conversation per (network, target), from irssi's `autolog_path`.
    //
    // `is_status` marks irssi's server-notice window. It is named after your own
    // nick, so it looks like a DM with yourself; the flag lets a reader leave it out
    // without knowing that nick.
    r"CREATE TABLE IF NOT EXISTS irc_conversations (
        id INT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        network VARCHAR(64) NOT NULL,
        target VARCHAR(255) NOT NULL,
        is_channel TINYINT(1) NOT NULL DEFAULT 0,
        is_status TINYINT(1) NOT NULL DEFAULT 0,
        updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
        UNIQUE KEY uniq_irc_conv (network, target)
    )",
    // v8: one row per logged line.
    //
    // `source_tag` is in the dedupe key because irssi tags a second simultaneous
    // connection `net2`, so one conversation-day can exist as two files. Without it
    // the second file's lines collide and INSERT IGNORE drops them.
    //
    // Seconds are always zero (irssi's default `%H:%M`); `id` keeps order within a
    // minute.
    r"CREATE TABLE IF NOT EXISTS irc_messages (
        id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        conversation_id INT NOT NULL,
        source_tag VARCHAR(64) NOT NULL,
        file_date DATE NOT NULL,
        line_no INT NOT NULL,
        sent_at DATETIME NOT NULL,
        nick VARCHAR(255) NULL,
        is_self TINYINT(1) NOT NULL DEFAULT 0,
        kind ENUM('message','action','event','notice') NOT NULL,
        text TEXT NULL,
        created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE KEY uniq_irc_line (conversation_id, source_tag, file_date, line_no),
        INDEX idx_irc_conv_ts (conversation_id, sent_at)
    )",
    // v9: answers the conversation list's per-conversation COUNT/MAX over
    // `kind IN ('message','action')` from the index alone. The optimizer picks it
    // only when `irc_messages` is aggregated in a derived table and then joined, and
    // the viewer's `src/archive/irc.rs` keeps that shape.
    //
    // `IF NOT EXISTS`: the live database had this index before this entry did.
    "ALTER TABLE irc_messages
        ADD INDEX IF NOT EXISTS idx_irc_conv_kind_ts (conversation_id, kind, sent_at)",
    // v10: files already imported, so a run reads only what changed.
    //
    // `(mtime, size)` is rsync's own quick-check: irssi logs are append-only, so a
    // change moves both, and a content hash would mean reading every file.
    //
    // A row is written only after the file's lines land, and only under `--apply`;
    // progress recorded by a dry run would make the next real run skip work.
    r"CREATE TABLE IF NOT EXISTS irc_import_state (
        rel_path VARCHAR(512) NOT NULL PRIMARY KEY,
        mtime_ns BIGINT NOT NULL,
        size_bytes BIGINT NOT NULL,
        imported_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
    )",
    // v11: per-conversation count and last time, maintained on write. The `kind`
    // filter defeats MariaDB's loose index scan and no query shape recovers it, so
    // the list reads this table instead of counting. Maintained rather than
    // refreshed because a lagging count shows in the UI.
    r"CREATE TABLE IF NOT EXISTS irc_conversation_stats (
        conversation_id INT NOT NULL PRIMARY KEY,
        cnt BIGINT NOT NULL DEFAULT 0,
        last_sent_at DATETIME NULL,
        updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
    )",
    // v12: a trigger rather than application code, so every writer maintains the
    // stats: the importer, `irc_tail`, and the viewer's IRC send echo.
    //
    // An ignored `INSERT IGNORE` fires no trigger, so replay cannot inflate a count.
    // Lines arrive out of timestamp order, hence `GREATEST`.
    //
    // A database that already holds lines needs a one-shot backfill, run with the
    // writers paused so no line is counted twice or not at all:
    //     DELETE FROM irc_conversation_stats;
    //     INSERT INTO irc_conversation_stats (conversation_id, cnt, last_sent_at)
    //     SELECT conversation_id, COUNT(*), MAX(sent_at) FROM irc_messages
    //      WHERE kind IN ('message','action') GROUP BY conversation_id;
    r"CREATE OR REPLACE TRIGGER trg_irc_stats_ai AFTER INSERT ON irc_messages FOR EACH ROW
    BEGIN
        IF NEW.kind IN ('message', 'action') THEN
            INSERT INTO irc_conversation_stats (conversation_id, cnt, last_sent_at)
                 VALUES (NEW.conversation_id, 1, NEW.sent_at)
            ON DUPLICATE KEY UPDATE
                 cnt = cnt + 1,
                 last_sent_at = GREATEST(COALESCE(last_sent_at, NEW.sent_at), NEW.sent_at);
        END IF;
    END",
    // v13: a trigger cannot recompute `MAX(sent_at)` after a delete (it may not
    // read its own table), so deletes are refused. To delete deliberately: drop this
    // trigger, delete, rebuild with v12's backfill, recreate it.
    r"CREATE OR REPLACE TRIGGER trg_irc_stats_bd BEFORE DELETE ON irc_messages FOR EACH ROW
    BEGIN
        SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT =
            'irc_messages is append-only: a DELETE would drift irc_conversation_stats';
    END",
    // v14: refuses only the updates that would drift the stats; `is_self`, `text`
    // and `nick` stay correctable.
    r"CREATE OR REPLACE TRIGGER trg_irc_stats_bu BEFORE UPDATE ON irc_messages FOR EACH ROW
    BEGIN
        IF NEW.conversation_id <> OLD.conversation_id
           OR NEW.kind <> OLD.kind
           OR NEW.sent_at <> OLD.sent_at THEN
            SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT =
                'changing conversation_id, kind or sent_at would drift irc_conversation_stats';
        END IF;
    END",
    // v15: Telegram conversations. `id` is the Bot-API normalisation, folding
    // MTProto's three id spaces into one:
    //
    //     user    →  user_id
    //     chat    → -chat_id                       (a basic group)
    //     channel → -1_000_000_000_000 - channel_id
    //
    // `src/telegram/map.rs::normalise_peer` implements it. `kind` is stored rather
    // than read off the sign.
    r"CREATE TABLE IF NOT EXISTS telegram_conversations (
        id BIGINT NOT NULL PRIMARY KEY,
        kind ENUM('dm','group','channel') NOT NULL,
        name VARCHAR(255) NULL,
        username VARCHAR(255) NULL,
        updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
    )",
    // v16: Telegram messages. `(conversation_id, msg_id)` is Telegram's own stable
    // identity, so backfill and live stream overlap safely under INSERT IGNORE.
    //
    // `sent_at` is unix seconds, as Telegram sends it; the viewer converts units.
    // `sender_name` is the name when the row landed, denormalised so the list needs
    // no join. `kind` separates service events from messages.
    r"CREATE TABLE IF NOT EXISTS telegram_messages (
        id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        conversation_id BIGINT NOT NULL,
        msg_id INT NOT NULL,
        sent_at BIGINT NOT NULL,
        sender_id BIGINT NULL,
        sender_name VARCHAR(255) NULL,
        is_outgoing TINYINT(1) NOT NULL DEFAULT 0,
        kind ENUM('message','service') NOT NULL DEFAULT 'message',
        text TEXT NULL,
        media_kind VARCHAR(32) NULL,
        edited_at BIGINT NULL,
        reply_to_msg_id INT NULL,
        fwd_from_name VARCHAR(255) NULL,
        deleted TINYINT(1) NOT NULL DEFAULT 0,
        deleted_at TIMESTAMP NULL,
        created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE KEY uniq_tg_msg (conversation_id, msg_id),
        INDEX idx_tg_conv_kind_ts (conversation_id, kind, sent_at)
    )",
    // v17: reaction counts per emoji; v31 records who.
    //
    // A custom emoji has no characters, so it is stored by `custom_emoji_id` with
    // `emoji` NULL. `reaction_key` is the one non-null identity of that pair: MariaDB
    // makes primary-key columns NOT NULL, a generated column cannot be the primary
    // key, and the `''` fallback stops NULLs escaping the unique key.
    //
    // `GENERATED ALWAYS AS … STORED` rather than MariaDB's `PERSISTENT`, because
    // dev-lint's DDL parser reads only the standard spelling.
    r"CREATE TABLE IF NOT EXISTS telegram_reactions (
        id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        conversation_id BIGINT NOT NULL,
        msg_id INT NOT NULL,
        emoji VARCHAR(32) NULL,
        custom_emoji_id BIGINT NULL,
        reaction_key VARCHAR(64) GENERATED ALWAYS AS
            (COALESCE(emoji, CONCAT('custom:', custom_emoji_id), '')) STORED,
        cnt INT NOT NULL DEFAULT 0,
        chosen TINYINT(1) NOT NULL DEFAULT 0,
        updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
        UNIQUE KEY uniq_tg_reaction (conversation_id, msg_id, reaction_key)
    )",
    // v18: text an edit replaced. Telegram edits in place under the same `msg_id`,
    // so the superseded text is appended here before the row is updated.
    //
    // `was_edited_at` is the replaced version's `edit_date`, NULL for the original.
    // The unique key makes replays idempotent; the NULL original is written only
    // while the stored `edited_at` is NULL.
    r"CREATE TABLE IF NOT EXISTS telegram_message_edits (
        id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        conversation_id BIGINT NOT NULL,
        msg_id INT NOT NULL,
        was_edited_at BIGINT NULL,
        text TEXT NULL,
        recorded_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE KEY uniq_tg_edit (conversation_id, msg_id, was_edited_at),
        INDEX idx_tg_edit_msg (conversation_id, msg_id)
    )",
    // v19: backfill progress, so a restart resumes. `oldest_seen` is the lowest
    // `msg_id` stored and the next page asks for older. `complete` is set only when a
    // page comes back empty, Telegram's one end-of-history signal.
    r"CREATE TABLE IF NOT EXISTS telegram_backfill_state (
        conversation_id BIGINT NOT NULL PRIMARY KEY,
        oldest_seen INT NULL,
        complete TINYINT(1) NOT NULL DEFAULT 0,
        messages_stored BIGINT NOT NULL DEFAULT 0,
        updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
    )",
    // v20: the MTProto session (auth key, DC list, peer cache, update state) as one
    // row. It is a credential: whoever reads it holds the account. Stored here it
    // shares the messages' backup and needs no writable volume.
    //
    // Losing it forces a re-login, which Telegram rate-limits for hours.
    // `single_row` prevents a second session, whose update stream would fight this
    // one. LONGTEXT because it is read and written whole.
    r"CREATE TABLE IF NOT EXISTS telegram_session (
        single_row TINYINT(1) NOT NULL PRIMARY KEY DEFAULT 1
            CHECK (single_row = 1),
        data LONGTEXT NOT NULL,
        updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
    )",
    // v21: size and mime, from the message itself with no download. NULL means not
    // yet known; the enrichment in `store_telegram_message` fills it.
    r"ALTER TABLE telegram_messages
        ADD COLUMN media_size BIGINT NULL,
        ADD COLUMN media_mime VARCHAR(128) NULL",
    // v22: Telegram's `edit_hide`: show the message as unmodified although it has an
    // `edit_date`. The edit is still recorded; this governs display. NULL, not 0, on
    // rows stored before the column, which were never told.
    r"ALTER TABLE telegram_messages ADD COLUMN edit_hidden TINYINT(1) NULL",
    // v23–v24: v21 made `media_kind` finer, but enrichment fills only NULLs, so rows
    // stored before it stay `document`. These relabel them. Idempotent.
    r"UPDATE telegram_messages SET media_kind = 'video'
       WHERE media_kind = 'document' AND media_mime LIKE 'video/%'",
    r"UPDATE telegram_messages SET media_kind = 'audio'
       WHERE media_kind = 'document' AND media_mime LIKE 'audio/%'",
    // v25: bytes this archive holds for a Telegram message; the media columns on
    // `telegram_messages` say what Telegram reported.
    //
    // Photos are fetched eagerly; larger media is `offered` and fetched when a reader
    // asks. `failed` keeps its reason in `note`.
    //
    // `stored_name` is a file name under `TELEGRAM_MEDIA_DIR`. The reader takes only
    // the name, so it cannot escape the mount.
    r"CREATE TABLE IF NOT EXISTS telegram_media (
        conversation_id BIGINT NOT NULL,
        msg_id INT NOT NULL,
        state ENUM('offered','stored','failed') NOT NULL,
        stored_name VARCHAR(255) NULL,
        size_bytes BIGINT NULL,
        content_type VARCHAR(128) NULL,
        note VARCHAR(255) NULL,
        requested_at TIMESTAMP NULL,
        stored_at TIMESTAMP NULL,
        updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
        PRIMARY KEY (conversation_id, msg_id),
        INDEX idx_tg_media_state (state, requested_at)
    )",
    // v26: a stat straight after `download_media` races the write and reads short or
    // zero. The size lives in `telegram_messages.media_size`.
    r"ALTER TABLE telegram_media DROP COLUMN size_bytes",
    // v27: `wanted` is the fetch queue. Only the feed holds a Telegram session and it
    // listens on no port, so the viewer writes a row and the feed polls;
    // `(state, requested_at)` makes the poll a lookup.
    r"ALTER TABLE telegram_media
        MODIFY COLUMN state ENUM('offered','wanted','stored','failed') NOT NULL",
    // v28: a forward's original sender. Telegram fills `from_name` only when that
    // account hides behind forward privacy; otherwise it sends `from_id`, normalised
    // like every other peer.
    r"ALTER TABLE telegram_messages
        ADD COLUMN fwd_from_id BIGINT NULL",
    // v29: a reaction that goes away is dated, not deleted. `removed_at IS NULL` is
    // current.
    r"ALTER TABLE telegram_reactions
        ADD COLUMN removed_at TIMESTAMP NULL",
    // v30: read marks. Telegram keeps only the current high-water marks, so a mark
    // not recorded as it happens is lost; each advance is its own row.
    //
    // `observed_at` is when we saw it: `updateReadHistoryOutbox` carries no date.
    // `direction` uses Telegram's words: `outbox` is how far they have read mine,
    // `inbox` how far I have read theirs.
    r"CREATE TABLE IF NOT EXISTS telegram_read_marks (
        id BIGINT AUTO_INCREMENT PRIMARY KEY,
        conversation_id BIGINT NOT NULL,
        direction ENUM('inbox','outbox') NOT NULL,
        max_id INT NOT NULL,
        observed_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE KEY uniq_tg_read (conversation_id, direction, max_id)
    ) DEFAULT CHARSET=utf8mb4",
    // v31: who reacted, and when, from `recent_reactions`. `telegram_reactions`
    // keeps the tally, which is authoritative. A list shorter than the tally is
    // truncated: it upserts whom it names and retracts nobody.
    //
    // `reacted_at` is Telegram's own date. For `reaction_key`, see v17.
    r"CREATE TABLE IF NOT EXISTS telegram_reaction_authors (
        id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        conversation_id BIGINT NOT NULL,
        msg_id INT NOT NULL,
        peer_id BIGINT NOT NULL,
        emoji VARCHAR(32) NULL,
        custom_emoji_id BIGINT NULL,
        reaction_key VARCHAR(64) GENERATED ALWAYS AS
            (COALESCE(emoji, CONCAT('custom:', custom_emoji_id), '')) STORED,
        reacted_at BIGINT NOT NULL,
        removed_at TIMESTAMP NULL,
        UNIQUE KEY uniq_tg_reaction_author (conversation_id, msg_id, peer_id, reaction_key),
        KEY idx_tg_reaction_author_peer (peer_id)
    ) DEFAULT CHARSET=utf8mb4",
    // v32: formatting entities, including links whose URL is not in the visible text
    // and mentions whose user id is the only record of who was meant.
    //
    // Offsets and lengths are UTF-16 code units, as Telegram counts. Identity is the
    // span, not the list position, so an inserted entity does not renumber the rest.
    r"CREATE TABLE IF NOT EXISTS telegram_message_entities (
        id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        conversation_id BIGINT NOT NULL,
        msg_id INT NOT NULL,
        kind VARCHAR(32) NOT NULL,
        offset_utf16 INT NOT NULL,
        length_utf16 INT NOT NULL,
        url TEXT NULL,
        user_id BIGINT NULL,
        language VARCHAR(32) NULL,
        document_id BIGINT NULL,
        removed_at TIMESTAMP NULL,
        UNIQUE KEY uniq_tg_entity (conversation_id, msg_id, kind, offset_utf16, length_utf16)
    ) DEFAULT CHARSET=utf8mb4",
    // v33: the TL constructor of a service action. `text` is our English rendering,
    // and an unknown action renders as "an event".
    r"ALTER TABLE telegram_messages
        ADD COLUMN service_action VARCHAR(64) NULL",
    // v34: how a call ended and how long it took. An unanswered call has no
    // duration.
    r"CREATE TABLE IF NOT EXISTS telegram_calls (
        id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        conversation_id BIGINT NOT NULL,
        msg_id INT NOT NULL,
        call_id BIGINT NULL,
        duration_s INT NULL,
        reason VARCHAR(32) NULL,
        video TINYINT(1) NOT NULL DEFAULT 0,
        UNIQUE KEY uniq_tg_call (conversation_id, msg_id)
    ) DEFAULT CHARSET=utf8mb4",
    // v35: `grouped_id` is the album. `fwd_date` is when the original was written.
    // `ttl_period` is the disappearing-message timer.
    r"ALTER TABLE telegram_messages
        ADD COLUMN grouped_id BIGINT NULL,
        ADD COLUMN fwd_date BIGINT NULL,
        ADD COLUMN fwd_channel_post INT NULL,
        ADD COLUMN via_bot_id BIGINT NULL,
        ADD COLUMN ttl_period INT NULL",
    // v36: a reply can quote a fragment of its target, and the target can be in
    // another conversation.
    r"ALTER TABLE telegram_messages
        ADD COLUMN reply_quote TEXT NULL,
        ADD COLUMN reply_to_peer_id BIGINT NULL",
    // v37: dead. It was inserted rather than appended, so databases that had already
    // recorded v37 skipped it; v40 is the live copy. Removing it would renumber every
    // later entry.
    r"CREATE TABLE IF NOT EXISTS signal_receipts (
        id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        target_ts BIGINT NOT NULL,
        author_uuid VARCHAR(64) NOT NULL,
        kind ENUM('delivery','read','viewed') NOT NULL,
        when_ts BIGINT NOT NULL,
        observed_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE KEY uniq_signal_receipt (target_ts, author_uuid, kind),
        INDEX idx_signal_receipt_target (target_ts)
    ) DEFAULT CHARSET=utf8mb4",
    // v38: Signal call signalling as it arrives: offer, answer, busy and hangup
    // frames sharing a `call_id`. Stored uninterpreted, since a duration needs both
    // ends to reach this device. `iceUpdateMessages` are left out: opaque transport,
    // many per call.
    r"CREATE TABLE IF NOT EXISTS signal_call_events (
        id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        call_id BIGINT NOT NULL,
        peer_uuid VARCHAR(64) NOT NULL,
        event ENUM('offer','answer','busy','hangup') NOT NULL,
        detail VARCHAR(32) NULL,
        device_id BIGINT NULL,
        event_ts BIGINT NOT NULL,
        observed_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE KEY uniq_signal_call_event (call_id, peer_uuid, event, event_ts),
        INDEX idx_signal_call (call_id)
    ) DEFAULT CHARSET=utf8mb4",
    // v39: re-capture progress, a forward walk through stored messages whose
    // enrichment fills columns added after they landed. Separate from
    // `telegram_backfill_state`, which walks older and whose `complete` means
    // something else. Deleting a row re-runs that conversation harmlessly.
    r"CREATE TABLE IF NOT EXISTS telegram_recapture_state (
        conversation_id BIGINT NOT NULL PRIMARY KEY,
        through_msg_id INT NOT NULL,
        updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
    ) DEFAULT CHARSET=utf8mb4",
    // v40: Signal delivery, read and viewed receipts. Signal sends each once, on the
    // live socket, and nothing restates it. One receipt covers many messages, so it
    // is flattened to a row per (message, author, kind).
    r"CREATE TABLE IF NOT EXISTS signal_receipts (
        id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        target_ts BIGINT NOT NULL,
        author_uuid VARCHAR(64) NOT NULL,
        kind ENUM('delivery','read','viewed') NOT NULL,
        when_ts BIGINT NOT NULL,
        observed_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE KEY uniq_signal_receipt (target_ts, author_uuid, kind),
        INDEX idx_signal_receipt_target (target_ts)
    ) DEFAULT CHARSET=utf8mb4",
    // v41–v42: `envelope.sourceName` is signal-cli's `getContactOrProfileName`:
    // nickname (0.14.7 and later), else system contact name, else profile name.
    r"ALTER TABLE contacts ADD COLUMN display_name VARCHAR(255) NULL",
    r"UPDATE contacts SET display_name = profile_name WHERE display_name IS NULL",
    // v43: names over time. A rename closes the current row and opens a new one, so
    // an old thread can show what someone was called then. `seen_from` on a
    // backfilled row is when the archive last touched it; Signal sends no such date.
    r"CREATE TABLE IF NOT EXISTS contact_names (
        id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        uuid VARCHAR(64) NOT NULL,
        name VARCHAR(255) NOT NULL,
        seen_from TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
        seen_until TIMESTAMP NULL,
        INDEX idx_contact_names_current (uuid, seen_until)
    ) DEFAULT CHARSET=utf8mb4",
    // v44: seeds the history with each contact's current name.
    r"INSERT INTO contact_names (uuid, name, seen_from)
        SELECT uuid, display_name, updated_at FROM contacts WHERE display_name IS NOT NULL",
    // v45: every frame as it arrived, before parsing. Signal says everything once
    // and the columns hold only part of it; any field can later be backfilled from
    // here, as v48 does.
    //
    // Keyed by content hash: a frame has no id, and signal-cli re-delivers on every
    // reconnect.
    r"CREATE TABLE IF NOT EXISTS signal_frames (
        id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        digest BINARY(32) NOT NULL,
        envelope_ts BIGINT NULL,
        source_uuid VARCHAR(64) NULL,
        frame JSON NOT NULL,
        received_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE KEY uniq_signal_frame (digest),
        INDEX idx_signal_frame_ts (envelope_ts),
        INDEX idx_signal_frame_source (source_uuid, envelope_ts)
    ) DEFAULT CHARSET=utf8mb4",
    // v46: `signal-ingester` is RollingUpdate, so this drop ships only after a
    // deploy that no longer writes the column.
    r"ALTER TABLE contacts DROP COLUMN profile_name",
    // v47: `server_received_ts` is Signal's clock. `server_ts` is the sender's and
    // doubles as the message's identity, so it cannot be corrected.
    // `expires_in_seconds`: NULL is no timer in the frame, 0 is the timer turned off.
    r"ALTER TABLE messages
        ADD COLUMN server_received_ts BIGINT NULL,
        ADD COLUMN server_delivered_ts BIGINT NULL,
        ADD COLUMN expires_in_seconds INT NULL",
    BACKFILL_SERVER_TIMES,
    // v49: what a quote says about its target, for a target the archive does
    // not hold.
    r"ALTER TABLE messages
        ADD COLUMN quote_author_uuid VARCHAR(64) NULL,
        ADD COLUMN quote_text TEXT NULL",
    // v50: styled runs of a message body, in Signal's own names. Positions are
    // UTF-16 code units; runs may overlap.
    r"CREATE TABLE IF NOT EXISTS signal_text_styles (
        id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        message_id BIGINT NOT NULL,
        style VARCHAR(16) NOT NULL,
        start_utf16 INT NOT NULL,
        length_utf16 INT NOT NULL,
        UNIQUE KEY uniq_signal_text_style (message_id, style, start_utf16, length_utf16)
    ) DEFAULT CHARSET=utf8mb4",
    // v51: link previews the sender's app attached, in the order it sent them.
    r"CREATE TABLE IF NOT EXISTS signal_link_previews (
        id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        message_id BIGINT NOT NULL,
        position INT NOT NULL,
        url TEXT NOT NULL,
        title TEXT NULL,
        description TEXT NULL,
        UNIQUE KEY uniq_signal_link_preview (message_id, position)
    ) DEFAULT CHARSET=utf8mb4",
    // v52–v54: backfill v49–v51 from `signal_frames`, as v48 does.
    BACKFILL_QUOTES,
    BACKFILL_TEXT_STYLES,
    BACKFILL_LINK_PREVIEWS,
    // v55: a preview's picture, an attachment the ingester stores like any other.
    r"ALTER TABLE signal_link_previews
        ADD COLUMN image_id VARCHAR(255) NULL,
        ADD COLUMN image_content_type VARCHAR(255) NULL,
        ADD COLUMN image_path VARCHAR(1024) NULL",
    // v56: backfill v55's names from `signal_frames`; the ingester fetches the bytes.
    BACKFILL_PREVIEW_IMAGES,
    // v57: who a body names, on the U+FFFC standing for them (UTF-16 code units).
    r"CREATE TABLE IF NOT EXISTS signal_mentions (
        id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        message_id BIGINT NOT NULL,
        start_utf16 INT NOT NULL,
        length_utf16 INT NOT NULL,
        uuid VARCHAR(64) NOT NULL,
        UNIQUE KEY uniq_signal_mention (message_id, start_utf16)
    ) DEFAULT CHARSET=utf8mb4",
    // v58: backfill v57 from `signal_frames`.
    BACKFILL_MENTIONS,
];
