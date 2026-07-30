#!/usr/bin/env bash
# Run a command against an ephemeral MariaDB, then tear it down.
#
#   scripts/with-test-db.sh cargo test
#
# Exports MESSAGES_TEST_DATABASE_URL so the end-to-end tests in tests/archive.rs
# actually run instead of skipping themselves. The datadir is a temp dir, wiped
# afterwards — those tests DROP and recreate the archive tables, so they must
# never see the real `signal` database.
#
# Needs mariadb on PATH — run inside `nix develop` (the flake dev shell carries
# it), which is what verify.sh does. Ported from fleetwatch, socket caveat and all.
set -euo pipefail

# 3318: fleetwatch's ephemeral DB already uses 3317, and the fleet gate can run
# both repos' verifies at once.
PORT="${MESSAGES_TEST_DB_PORT:-3318}"
DBDIR="$(mktemp -d "${TMPDIR:-/tmp}/messages-test-db.XXXXXX")"

# The socket lives OUTSIDE the datadir, in a short path of its own.
#
# A Unix socket path is capped at 103 bytes by the kernel, and $TMPDIR is not
# short when this runs under nested nix-shells: `~/Code/check --full` invokes
# this repo's verify inside its own shell, giving a $TMPDIR like
# /private/tmp/nix-shell-<pid>-<n>/nix-shell.XXXXXX/nix-shell.XXXXXX/. The
# datadir has no such limit, so only the socket needs to escape.
SOCKDIR="$(mktemp -d /tmp/msg-sock.XXXXXX)"
SOCKET="$SOCKDIR/d.sock"
if [ ${#SOCKET} -gt 103 ]; then
    echo "socket path is ${#SOCKET} bytes, over the 103 the kernel allows: $SOCKET" >&2
    exit 1
fi

cleanup() {
    [ -n "${DB_PID:-}" ] && kill "$DB_PID" 2>/dev/null && wait "$DB_PID" 2>/dev/null
    rm -rf "$DBDIR" "$SOCKDIR"
}
trap cleanup EXIT

mariadb-install-db --no-defaults --datadir="$DBDIR/data" \
    --auth-root-authentication-method=normal >/dev/null

cat >"$DBDIR/init.sql" <<'SQL'
CREATE DATABASE IF NOT EXISTS messages_test CHARACTER SET utf8mb4;
CREATE USER IF NOT EXISTS 'messages'@'127.0.0.1' IDENTIFIED BY 'messages';
GRANT ALL PRIVILEGES ON messages_test.* TO 'messages'@'127.0.0.1';
FLUSH PRIVILEGES;
SQL

# --skip-name-resolve: match by numeric IP so the '127.0.0.1' grant applies.
mariadbd --no-defaults --datadir="$DBDIR/data" --socket="$SOCKET" \
    --port="$PORT" --bind-address=127.0.0.1 --skip-name-resolve \
    --init-file="$DBDIR/init.sql" >"$DBDIR/mariadbd.log" 2>&1 &
DB_PID=$!

for _ in $(seq 1 100); do
    if mariadb-admin --no-defaults --socket="$SOCKET" -u root ping >/dev/null 2>&1; then
        break
    fi
    if ! kill -0 "$DB_PID" 2>/dev/null; then
        echo "mariadbd died during startup:" >&2
        cat "$DBDIR/mariadbd.log" >&2
        exit 1
    fi
    sleep 0.2
done

export MESSAGES_TEST_DATABASE_URL="mysql://messages:messages@127.0.0.1:$PORT/messages_test"
"$@"
