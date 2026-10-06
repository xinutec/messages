#!/usr/bin/env nix-shell
#!nix-shell -i python3 -p "python3.withPackages(ps: [ps.pymysql])"
"""Import the Google Chat archive into its OWN tables in the signal MariaDB.

Source is the decoded archive produced by ~/Code/gchat-archive (not Takeout):
each `conversations/<group_id>.json` has {group_id, name, message_count, messages[]},
and each message has {msg_id, thread_id, sender_id, sender_name, text, ts, ts_raw,
reactions[{emoji, count, reactors[{id, name}]}]}. `sender_name` carries a trailing
" (you)" for self.

`reactors` comes from a second rpc sync.py replays per reacted message; a
reaction without it is unresolved, not unreacted.

Writes SQL, one transaction, to stdout, and a summary to stderr. It never
connects: the database's own client runs it, with the password its pod holds.

Idempotent: messages dedupe on (group_id, msg_id) via INSERT IGNORE; conversation
names and reaction counts are upserted, so re-running picks up a fresh export.

Usage:
    ./import_gchat.py [conversations_dir] \\
      | ssh root@isis kubectl -n signal exec -i deploy/signal-db -- \\
          sh -c 'mariadb --default-character-set=utf8mb4 -uroot -p"$MARIADB_ROOT_PASSWORD" signal'
The dir defaults to ~/Code/gchat-archive/archive/conversations. Without the
pipe it is a dry run. The client must be told utf8mb4, or emoji arrive mangled.
"""
import datetime as dt
import glob
import json
import os
import sys

from pymysql.converters import escape_item

DDL = [
    """CREATE TABLE IF NOT EXISTS gchat_conversations (
        group_id   VARCHAR(64) NOT NULL PRIMARY KEY,
        name       VARCHAR(255) NULL,
        is_dm      TINYINT(1) NOT NULL DEFAULT 0,
        updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
    ) DEFAULT CHARSET=utf8mb4""",
    """CREATE TABLE IF NOT EXISTS gchat_messages (
        id          BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        group_id    VARCHAR(64) NOT NULL,
        msg_id      VARCHAR(64) NOT NULL,
        thread_id   VARCHAR(64) NULL,
        reply_to_msg_id VARCHAR(64) NULL,
        sender_id   VARCHAR(32) NULL,
        sender_name VARCHAR(255) NULL,
        is_self     TINYINT(1) NOT NULL DEFAULT 0,
        ts_us       BIGINT NOT NULL,
        sent_at     DATETIME(6) NULL,
        text        TEXT NULL,
        created_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE KEY uniq_gchat_msg (group_id, msg_id),
        INDEX idx_gchat_conv_ts (group_id, ts_us)
    ) DEFAULT CHARSET=utf8mb4""",
    """CREATE TABLE IF NOT EXISTS gchat_attachments (
        id         BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        message_id BIGINT NOT NULL,
        name       VARCHAR(255) NULL,
        mime       VARCHAR(128) NULL,
        width      INT NULL,
        height     INT NULL,
        -- Not a UUID: Google Chat can derive it from a file name, well over 64
        -- characters, and the session has no strict mode to refuse truncation.
        uuid       VARCHAR(255) NULL,
        token      TEXT NULL,
        hash1      VARCHAR(128) NULL,
        hash2      VARCHAR(128) NULL,
        UNIQUE KEY uniq_gchat_attachment (message_id, uuid),
        INDEX idx_gchat_attachment_msg (message_id)
    ) DEFAULT CHARSET=utf8mb4""",
    # NULL until gchat-archive's fetch_attachments.py holds the bytes.
    """ALTER TABLE gchat_attachments
        ADD COLUMN IF NOT EXISTS stored_path VARCHAR(255) NULL""",
    """CREATE TABLE IF NOT EXISTS gchat_reaction_authors (
        id         BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        message_id BIGINT NOT NULL,
        emoji      VARCHAR(64) NOT NULL,
        reactor_id VARCHAR(32) NOT NULL,
        UNIQUE KEY uniq_gchat_reactor (message_id, emoji, reactor_id),
        INDEX idx_gchat_reactor_msg (message_id)
    ) DEFAULT CHARSET=utf8mb4""",
    """CREATE TABLE IF NOT EXISTS gchat_reactions (
        id         BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
        message_id BIGINT NOT NULL,
        emoji      VARCHAR(64) NULL,
        cnt        INT NOT NULL DEFAULT 0,
        UNIQUE KEY uniq_gchat_reaction (message_id, emoji),
        INDEX idx_gchat_react_msg (message_id)
    ) DEFAULT CHARSET=utf8mb4""",
]


def self_split(sender_name):
    """Strip the trailing " (you)" self-marker; return (display_name, is_self)."""
    if sender_name and sender_name.endswith(" (you)"):
        return sender_name[: -len(" (you)")], 1
    return sender_name, 0


class Script:
    """SQL for the database's own client, written as it is executed: each
    statement with its parameters as MariaDB literals. A cursor's `execute`, so
    dev-lint judges these statements against the schema as it would a cursor's."""

    def __init__(self, out):
        self.out = out

    def execute(self, stmt, params=()):
        self.out.write(stmt % tuple(escape_item(p, "utf8mb4") for p in params) + ";\n")


def write(cur, conv_dir, stats):
    """The import, as one transaction. A message's attachments and reactions
    find its row by (group_id, msg_id), so a duplicate finds the row already
    there."""
    cur.execute("START TRANSACTION")
    for stmt in DDL:
        cur.execute(stmt)

    # What fetch_attachments.py pulled, by the key it wrote.
    stored = {}
    by_msg = os.path.join(os.path.dirname(conv_dir), "attachments", "by_message.json")
    if os.path.exists(by_msg):
        with open(by_msg) as fh:
            stored = json.load(fh)

    for path in sorted(glob.glob(os.path.join(conv_dir, "*.json"))):
        with open(path) as f:
            conv = json.load(f)
        gid = conv.get("group_id")
        if not gid:
            continue
        name = conv.get("name") or None
        is_dm = 1 if (name or "").startswith("DM with ") else 0
        stats["conversations"] += 1
        cur.execute(
            "INSERT INTO gchat_conversations (group_id, name, is_dm) VALUES (%s,%s,%s) "
            "ON DUPLICATE KEY UPDATE name=COALESCE(VALUES(name), name), is_dm=VALUES(is_dm)",
            (gid, name, is_dm))

        for m in conv.get("messages", []):
            msg_id = m.get("msg_id")
            ts_raw = m.get("ts_raw")
            if not msg_id or not ts_raw:
                stats["skipped"] += 1
                continue
            ts_us = int(ts_raw)
            sent_at = dt.datetime.fromtimestamp(ts_us / 1_000_000, dt.timezone.utc).replace(tzinfo=None)
            disp, is_self = self_split(m.get("sender_name"))
            stats["messages"] += 1

            cur.execute(
                # Not `thread_id`: that is the topic; this is the message
                # answered. DMs have quote-replies but no topics.
                "INSERT IGNORE INTO gchat_messages "
                "(group_id, msg_id, thread_id, reply_to_msg_id, sender_id, sender_name, "
                " is_self, ts_us, sent_at, text) "
                "VALUES (%s,%s,%s,%s,%s,%s,%s,%s,%s,%s)",
                (gid, msg_id, m.get("thread_id"),
                 (m.get("reply_to") or {}).get("msg_id"),
                 m.get("sender_id"), disp, is_self,
                 ts_us, sent_at, m.get("text")))

            # Attachments. The bytes need the user's session: the client mints the
            # download URL from `token` at render time. The hashes match bytes
            # fetched later back to their row.
            for a in m.get("attachments") or []:
                stats["attachments"] += 1
                if a.get("uuid"):
                    cur.execute(
                        "INSERT IGNORE INTO gchat_attachments "
                        "(message_id, name, mime, width, height, uuid, token, hash1, hash2) "
                        "SELECT id,%s,%s,%s,%s,%s,%s,%s,%s FROM gchat_messages "
                        "WHERE group_id=%s AND msg_id=%s",
                        (a.get("name"), a.get("mime"), a.get("width"), a.get("height"),
                         a.get("uuid"), a.get("token"), a.get("hash1"), a.get("hash2"),
                         gid, msg_id))
                    cur.execute(
                        "UPDATE gchat_attachments x JOIN gchat_messages m ON m.id = x.message_id "
                        "SET x.name=%s, x.mime=%s, x.width=%s, x.height=%s, x.token=%s "
                        "WHERE m.group_id=%s AND m.msg_id=%s AND x.uuid=%s",
                        (a.get("name"), a.get("mime"), a.get("width"), a.get("height"),
                         a.get("token"), gid, msg_id, a.get("uuid")))
                else:
                    # The unique key lets NULLs repeat, so without a uuid an
                    # attachment is its message and name.
                    cur.execute(
                        "INSERT INTO gchat_attachments "
                        "(message_id, name, mime, width, height, uuid, token, hash1, hash2) "
                        "SELECT m.id,%s,%s,%s,%s,NULL,%s,%s,%s FROM gchat_messages m "
                        "WHERE m.group_id=%s AND m.msg_id=%s AND NOT EXISTS "
                        "(SELECT 1 FROM gchat_attachments x "
                        " WHERE x.message_id=m.id AND x.uuid IS NULL AND x.name <=> %s)",
                        (a.get("name"), a.get("mime"), a.get("width"), a.get("height"),
                         a.get("token"), a.get("hash1"), a.get("hash2"),
                         gid, msg_id, a.get("name")))
                    cur.execute(
                        "UPDATE gchat_attachments x JOIN gchat_messages m ON m.id = x.message_id "
                        "SET x.mime=%s, x.width=%s, x.height=%s, x.token=%s "
                        "WHERE m.group_id=%s AND m.msg_id=%s AND x.uuid IS NULL AND x.name <=> %s",
                        (a.get("mime"), a.get("width"), a.get("height"), a.get("token"),
                         gid, msg_id, a.get("name")))
                # The bytes, if fetched, keyed on (group, message, uuid).
                held = stored.get(f"{gid}\t{msg_id}\t{a.get('uuid')}")
                if held:
                    # `<=>`: some attachments have no uuid.
                    cur.execute(
                        "UPDATE gchat_attachments x JOIN gchat_messages m ON m.id = x.message_id "
                        "SET x.stored_path=%s "
                        "WHERE m.group_id=%s AND m.msg_id=%s AND x.uuid <=> %s",
                        (held["file"], gid, msg_id, a.get("uuid")))

            for r in m.get("reactions") or []:
                emoji = r.get("emoji")
                if not emoji:
                    continue
                stats["reactions"] += 1
                count = int(r.get("count") or 0)
                cur.execute(
                    "INSERT IGNORE INTO gchat_reactions (message_id, emoji, cnt) "
                    "SELECT id,%s,%s FROM gchat_messages WHERE group_id=%s AND msg_id=%s",
                    (emoji, count, gid, msg_id))
                cur.execute(
                    "UPDATE gchat_reactions x JOIN gchat_messages m ON m.id = x.message_id "
                    "SET x.cnt=%s WHERE m.group_id=%s AND m.msg_id=%s AND x.emoji=%s",
                    (count, gid, msg_id, emoji))

                # Who reacted. Rows are only added: an absent or short
                # `reactors` is unresolved, not a retraction.
                for who in (r.get("reactors") or []):
                    rid = who.get("id") if isinstance(who, dict) else who
                    if not rid:
                        continue
                    stats["reactors"] += 1
                    cur.execute(
                        "INSERT IGNORE INTO gchat_reaction_authors "
                        "(message_id, emoji, reactor_id) "
                        "SELECT id,%s,%s FROM gchat_messages WHERE group_id=%s AND msg_id=%s",
                        (emoji, str(rid), gid, msg_id))
    cur.execute("COMMIT")


def main():
    conv_dir = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser(
        "~/Code/gchat-archive/archive/conversations")
    if not glob.glob(os.path.join(conv_dir, "*.json")):
        sys.exit(f"no conversation JSON found in {conv_dir}")
    stats = {"conversations": 0, "messages": 0, "reactions": 0,
             "reactors": 0, "attachments": 0, "skipped": 0}
    write(Script(sys.stdout), conv_dir, stats)
    print(f"done: {stats} (messages counts duplicates too)", file=sys.stderr)


if __name__ == "__main__":
    main()
