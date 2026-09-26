#!/usr/bin/env nix-shell
#!nix-shell -i python3 -p "python3.withPackages(ps: [ps.pymysql])"
"""Check the archive still answers the things Pippijn knows to be true.

Each row of `known_truths.tsv` and `known_truths.private.tsv` is known
independently of the archive, so a query that stops returning it means the
pipeline lost something. `expect` is a floor, since the counts only grow; a
query about an absence counts it as `= 0`, so it too answers 1 when true.

The private file is git-crypt'd. In a clone without the key it is ciphertext,
and only the public rows are checked, which the output says.

Usage (env: DB_HOST DB_PORT DB_USER DB_PASSWORD DB_NAME):

    ./check_known_truths.py [--sql]

`--sql` prints the queries instead, for running where the database is reachable.
"""
import os
import sys
import pathlib

HERE = pathlib.Path(__file__).resolve().parent
TRUTHS = [HERE / "known_truths.tsv", HERE / "known_truths.private.tsv"]


def readable(path):
    """The file's text, or None while git-crypt still has it encrypted."""
    data = path.read_bytes()
    if data.startswith(b"\x00GITCRYPT"):
        print(f"{path.name} is encrypted (no git-crypt key here); its rows are not checked",
              file=sys.stderr)
        return None
    return data.decode()


def rows():
    for path in TRUTHS:
        text = readable(path)
        if text is not None:
            yield from rows_of(text)


def rows_of(text):
    for line in text.splitlines():
        if not line.strip() or line.startswith("#") or line.startswith("name\t"):
            continue
        parts = line.split("\t")
        if len(parts) != 3:
            print(f"malformed row: {line[:60]}", file=sys.stderr)
            continue
        name, expect, sql = parts
        yield name.strip(), int(expect), sql.strip()


def main():
    if "--sql" in sys.argv:
        for name, expect, sql in rows():
            # The name as a literal, so the output labels each number.
            print(f"SELECT {expect} AS expect, ({sql}) AS actual, "
                  f"{expect} <= ({sql}) AS ok, '{name.replace(chr(39), '')}' AS what;")
        return

    import pymysql

    conn = pymysql.connect(
        host=os.environ["DB_HOST"], port=int(os.environ.get("DB_PORT", "3306")),
        user=os.environ["DB_USER"], password=os.environ["DB_PASSWORD"],
        database=os.environ["DB_NAME"], charset="utf8mb4")
    cur = conn.cursor()
    bad = 0
    for name, expect, sql in rows():
        cur.execute(sql)
        got = cur.fetchone()[0]
        ok = got >= expect
        bad += 0 if ok else 1
        print(f"{'ok  ' if ok else 'LOST'}  {got:>6} (>= {expect:<6}) {name}")
    conn.close()
    if bad:
        print(f"\n{bad} truth(s) the archive can no longer answer", file=sys.stderr)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
