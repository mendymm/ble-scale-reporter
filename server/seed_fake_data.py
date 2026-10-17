#!/usr/bin/env python3

import argparse
import math
import random
import sqlite3
from datetime import UTC, datetime, timedelta
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Seed fake BLE scale reports")
    parser.add_argument(
        "--data-dir",
        type=Path,
        default=Path("/tmp/fake-data"),
        help="server data directory (default: /tmp/fake-data)",
    )
    parser.add_argument("--days", type=int, default=120)
    parser.add_argument("--start-weight", type=float, default=82.4)
    parser.add_argument("--end-weight", type=float, default=79.1)
    parser.add_argument("--seed", type=int, default=42)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    if args.days < 2:
        raise SystemExit("--days must be at least 2")

    args.data_dir.mkdir(parents=True, exist_ok=True)
    database_path = args.data_dir / "reports.sqlite"
    random_source = random.Random(args.seed)
    final_day = datetime.now(UTC).replace(hour=7, minute=30, second=0, microsecond=0)
    first_day = final_day - timedelta(days=args.days - 1)

    reports: list[tuple[int, int]] = []
    for day in range(args.days):
        # Skip an occasional day so the time axis is not perfectly uniform.
        if day not in (0, args.days - 1) and random_source.random() < 0.08:
            continue

        progress = day / (args.days - 1)
        trend = args.start_weight + (args.end_weight - args.start_weight) * progress
        weekly_variation = math.sin(day * math.tau / 7) * 0.18
        noise = random_source.gauss(0, 0.16)
        weight_hundredths = round((trend + weekly_variation + noise) * 100)
        timestamp = int((first_day + timedelta(days=day)).timestamp())
        reports.append((timestamp, weight_hundredths))

    with sqlite3.connect(database_path) as connection:
        connection.execute("PRAGMA journal_mode = WAL")
        connection.execute(
            """
            CREATE TABLE IF NOT EXISTS reports (
                timestamp INTEGER PRIMARY KEY,
                weight INTEGER NOT NULL CHECK (weight BETWEEN 0 AND 4294967295),
                samples TEXT NOT NULL DEFAULT '[]'
            ) STRICT
            """
        )
        columns = {
            row[1] for row in connection.execute("PRAGMA table_info(reports)")
        }
        if "samples" not in columns:
            connection.execute(
                "ALTER TABLE reports ADD COLUMN samples TEXT NOT NULL DEFAULT '[]'"
            )
        connection.executemany(
            """
            INSERT INTO reports (timestamp, weight, samples)
            VALUES (?, ?, '[]')
            ON CONFLICT(timestamp) DO UPDATE SET
                weight = excluded.weight,
                samples = excluded.samples
            """,
            reports,
        )

    print(f"Wrote {len(reports)} reports to {database_path}")


if __name__ == "__main__":
    main()
