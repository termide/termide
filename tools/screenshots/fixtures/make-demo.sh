#!/usr/bin/env bash
# Build the synthetic project every screenshot shows, under /tmp/demo.
# Everything here is invented: names, data, history, author. Runs inside the
# recording container only; the dates are fixed so each run is identical.
set -euo pipefail

ROOT=/tmp/demo
PROJECT=$ROOT/inventory
rm -rf "$ROOT"
mkdir -p "$PROJECT"
cd "$PROJECT"

commit() {
    # commit <iso-date> <message>
    GIT_AUTHOR_DATE="$1" GIT_COMMITTER_DATE="$1" git commit -q -m "$2"
}

git init -q -b main

# --- 1. skeleton -------------------------------------------------------------
mkdir -p inventory tests docs
cat > README.md <<'EOF'
# Inventory

A small stock-keeping service for a chain of hardware stores: items, stock
levels per store, purchase orders and a daily reorder report.

## Quick start

```sh
python3 -m inventory.cli report --store north
python3 -m unittest -q
```

## How an order flows

```mermaid
flowchart LR
    A[Count] --> B{Low?}
    B -- yes --> C[Report]
    C --> D[Order]
    D --> E[Supplier]
    B -- no --> F[Skip]
```

## Layout

| Path | What it holds |
|------|---------------|
| `inventory/models.py` | Items, stores and stock levels |
| `inventory/report.py` | The daily reorder report |
| `inventory/cli.py` | Command-line entry point |
| `data/inventory.db` | Sample SQLite database |

Stock levels are counted nightly; the report runs at 06:00 UTC.
EOF

cat > inventory/__init__.py <<'EOF'
"""Stock keeping for a small chain of hardware stores."""

__version__ = "0.4.0"
EOF

cat > inventory/models.py <<'EOF'
from dataclasses import dataclass, field
from decimal import Decimal


@dataclass(frozen=True)
class Item:
    sku: str
    name: str
    unit_price: Decimal
    on_hand: int
    reorder_point: int
    supplier: str = "Acme Supply"

    @property
    def needs_reorder(self) -> bool:
        return self.on_hand < self.reorder_point

    @property
    def stock_value(self) -> Decimal:
        return self.unit_price * self.on_hand


@dataclass
class Store:
    code: str
    city: str
    items: list[Item] = field(default_factory=list)

    def find(self, sku: str) -> Item | None:
        return next((item for item in self.items if item.sku == sku), None)

    def total_value(self) -> Decimal:
        return sum((item.stock_value for item in self.items), Decimal("0"))
EOF

cat > inventory/report.py <<'EOF'
"""The daily reorder report: what each store should order today."""

import csv
import io

from .models import Store


def reorder_rows(store: Store) -> list[tuple]:
    """Items below their reorder point, most urgent first."""
    rows = []
    for item in sorted(store.items, key=lambda i: i.on_hand - i.reorder_point):
        if item.needs_reorder:
            rows.append((item.sku, item.name, item.on_hand, item.reorder_point))
    return rows


def to_csv(rows: list[tuple]) -> str:
    buffer = io.StringIO()
    writer = csv.writer(buffer)
    writer.writerow(["sku", "name", "on_hand", "reorder_point"])
    writer.writerows(rows)
    return buffer.getvalue()


def render(store: Store) -> str:
    rows = reorder_rows(store)
    if not rows:
        return f"{store.city}: nothing to reorder"
    lines = [f"{store.city}: {len(rows)} item(s) to reorder"]
    for sku, name, on_hand, point, *_ in rows:
        lines.append(f"  {sku:<10} {name:<28} {on_hand:>4} / {point}")
    return "\n".join(lines)
EOF

cat > inventory/cli.py <<'EOF'
import argparse
import sqlite3
from decimal import Decimal
from pathlib import Path

from .models import Item, Store
from .report import render

DB = Path(__file__).resolve().parent.parent / "data" / "inventory.db"


def load_store(code: str) -> Store:
    with sqlite3.connect(DB) as conn:
        city = conn.execute("SELECT city FROM stores WHERE code = ?", (code,)).fetchone()[0]
        rows = conn.execute(
            "SELECT i.sku, i.name, i.unit_price, s.on_hand, i.reorder_point, i.supplier "
            "FROM items i JOIN stock s ON s.sku = i.sku WHERE s.store = ?",
            (code,),
        ).fetchall()
    items = [Item(sku, name, Decimal(str(price)), on_hand, point, supplier)
             for sku, name, price, on_hand, point, supplier in rows]
    return Store(code, city, items)


def main() -> None:
    parser = argparse.ArgumentParser(prog="inventory")
    sub = parser.add_subparsers(dest="command", required=True)
    report = sub.add_parser("report", help="print today's reorder report")
    report.add_argument("--store", default="north")
    args = parser.parse_args()
    if args.command == "report":
        print(render(load_store(args.store)))


if __name__ == "__main__":
    main()
EOF

cat > tests/__init__.py <<'EOF'
EOF

cat > tests/test_report.py <<'EOF'
import unittest
from decimal import Decimal

from inventory.models import Item, Store
from inventory.report import reorder_rows, render, to_csv


def store() -> Store:
    return Store("north", "Northfield", [
        Item("HX-1001", "Hex bolt M8x40 (box of 50)", Decimal("7.90"), 3, 20),
        Item("HX-1002", "Wing nut M8 (box of 100)", Decimal("5.40"), 40, 25),
        Item("PT-2210", "Exterior paint, white 5 l", Decimal("39.00"), 6, 8),
    ])


class ReorderTest(unittest.TestCase):
    def test_only_items_below_the_point(self):
        self.assertEqual([r[0] for r in reorder_rows(store())], ["HX-1001", "PT-2210"])

    def test_most_urgent_first(self):
        self.assertEqual(reorder_rows(store())[0][0], "HX-1001")

    def test_csv_has_a_header(self):
        self.assertTrue(to_csv(reorder_rows(store())).startswith("sku,name"))

    def test_render_counts_items(self):
        self.assertIn("2 item(s)", render(store()))

    def test_empty_store(self):
        self.assertIn("nothing to reorder", render(Store("east", "Eastbrook")))

    def test_stock_value(self):
        self.assertEqual(store().total_value(), Decimal("473.70"))


if __name__ == "__main__":
    unittest.main()
EOF

cat > .gitignore <<'EOF'
__pycache__/
*.pyc
.termide/sessions/
EOF

git add -A
commit "2026-08-03T09:12:00Z" "Initial item and store models"

# --- 2. data -------------------------------------------------------------------
mkdir -p data
sqlite3 data/inventory.db <<'EOF'
CREATE TABLE stores (code TEXT PRIMARY KEY, city TEXT NOT NULL, opened DATE);
CREATE TABLE items (
    sku TEXT PRIMARY KEY, name TEXT NOT NULL, category TEXT,
    unit_price REAL NOT NULL, reorder_point INTEGER NOT NULL, supplier TEXT
);
CREATE TABLE stock (store TEXT, sku TEXT, on_hand INTEGER, counted_at TEXT,
    PRIMARY KEY (store, sku));
CREATE TABLE orders (
    id INTEGER PRIMARY KEY, store TEXT, sku TEXT, quantity INTEGER,
    status TEXT, created_at TEXT
);
INSERT INTO stores VALUES
 ('north','Northfield','2019-04-01'),('east','Eastbrook','2020-09-15'),
 ('south','Southport','2021-03-02'),('west','Westhaven','2023-06-20');
INSERT INTO items VALUES
 ('HX-1001','Hex bolt M8x40 (box of 50)','Fasteners',7.90,20,'Acme Supply'),
 ('HX-1002','Wing nut M8 (box of 100)','Fasteners',5.40,25,'Acme Supply'),
 ('HX-1040','Wood screw 4x50 (box of 200)','Fasteners',9.80,30,'Acme Supply'),
 ('PT-2210','Exterior paint, white 5 l','Paint',39.00,8,'Colorline'),
 ('PT-2211','Exterior paint, slate 5 l','Paint',41.50,6,'Colorline'),
 ('PT-2300','Primer, universal 2.5 l','Paint',18.20,10,'Colorline'),
 ('TL-3105','Cordless drill 18 V','Tools',129.00,4,'Voltworks'),
 ('TL-3120','Impact driver 18 V','Tools',149.00,3,'Voltworks'),
 ('TL-3300','Spirit level 60 cm','Tools',17.50,6,'Voltworks'),
 ('GD-4001','Garden hose 25 m','Garden',34.90,5,'Greenline'),
 ('GD-4010','Pruning shears','Garden',21.00,8,'Greenline'),
 ('EL-5002','Extension cord 10 m','Electrical',16.40,12,'Voltworks'),
 ('EL-5050','LED bulb E27 9 W (4-pack)','Electrical',11.90,24,'Brightco'),
 ('PL-6007','PVC pipe 32 mm x 2 m','Plumbing',6.30,15,'Flowmax'),
 ('PL-6020','Ball valve 1/2"','Plumbing',8.70,10,'Flowmax');
INSERT INTO stock VALUES
 ('north','HX-1001',3,'2026-09-26 22:00'),('north','HX-1002',40,'2026-09-26 22:00'),
 ('north','HX-1040',12,'2026-09-26 22:00'),('north','PT-2210',6,'2026-09-26 22:00'),
 ('north','PT-2211',9,'2026-09-26 22:00'),('north','PT-2300',2,'2026-09-26 22:00'),
 ('north','TL-3105',5,'2026-09-26 22:00'),('north','TL-3120',1,'2026-09-26 22:00'),
 ('north','TL-3300',14,'2026-09-26 22:00'),('north','GD-4001',7,'2026-09-26 22:00'),
 ('north','GD-4010',8,'2026-09-26 22:00'),('north','EL-5002',30,'2026-09-26 22:00'),
 ('north','EL-5050',11,'2026-09-26 22:00'),('north','PL-6007',44,'2026-09-26 22:00'),
 ('north','PL-6020',4,'2026-09-26 22:00'),
 ('east','HX-1001',55,'2026-09-26 22:00'),('east','PT-2210',2,'2026-09-26 22:00'),
 ('east','TL-3105',0,'2026-09-26 22:00'),('east','GD-4001',12,'2026-09-26 22:00');
INSERT INTO orders (store, sku, quantity, status, created_at) VALUES
 ('north','HX-1001',50,'delivered','2026-09-01 06:04'),
 ('north','PT-2300',12,'delivered','2026-09-03 06:02'),
 ('east','TL-3105',6,'in transit','2026-09-20 06:01'),
 ('north','TL-3120',4,'in transit','2026-09-22 06:03'),
 ('south','GD-4010',10,'ordered','2026-09-24 06:00'),
 ('west','EL-5050',48,'ordered','2026-09-25 06:02'),
 ('north','PL-6020',15,'ordered','2026-09-26 06:01'),
 ('east','PT-2210',16,'draft','2026-09-27 06:00');
EOF
# A stock count exported from the stores, for the CSV table viewer.
python3 - <<'PY'
import csv
items = [
    ("HX-1001", "Hex bolt M8x40 (box of 50)", "Fasteners", 7.90, 20),
    ("HX-1002", "Wing nut M8 (box of 100)", "Fasteners", 5.40, 25),
    ("HX-1040", "Wood screw 4x50 (box of 200)", "Fasteners", 9.80, 30),
    ("PT-2210", "Exterior paint, white 5 l", "Paint", 39.00, 8),
    ("PT-2211", "Exterior paint, slate 5 l", "Paint", 41.50, 6),
    ("PT-2300", "Primer, universal 2.5 l", "Paint", 18.20, 10),
    ("TL-3105", "Cordless drill 18 V", "Tools", 129.00, 4),
    ("TL-3120", "Impact driver 18 V", "Tools", 149.00, 3),
    ("TL-3300", "Spirit level 60 cm", "Tools", 17.50, 6),
    ("GD-4001", "Garden hose 25 m", "Garden", 34.90, 5),
    ("GD-4010", "Pruning shears", "Garden", 21.00, 8),
    ("EL-5002", "Extension cord 10 m", "Electrical", 16.40, 12),
    ("EL-5050", "LED bulb E27 9 W (4-pack)", "Electrical", 11.90, 24),
    ("PL-6007", "PVC pipe 32 mm x 2 m", "Plumbing", 6.30, 15),
    ("PL-6020", 'Ball valve 1/2"', "Plumbing", 8.70, 10),
]
stores = [("north", "22:00"), ("east", "21:30"), ("south", "22:15"), ("west", "21:45")]
with open("data/stock-count.csv", "w", newline="") as out:
    writer = csv.writer(out)
    writer.writerow(["store", "sku", "item", "category", "on_hand",
                     "reorder_point", "unit_price", "stock_value", "counted_at"])
    for s, (store, time) in enumerate(stores):
        for i, (sku, name, category, price, reorder) in enumerate(items):
            on_hand = (i * 17 + s * 29 + 7) % 61
            writer.writerow([store, sku, name, category, on_hand, reorder,
                             f"{price:.2f}", f"{on_hand * price:.2f}",
                             f"2026-09-26 {time}"])
PY
git add -A
commit "2026-08-05T14:40:00Z" "Add sample SQLite database"

cat > docs/architecture.mmd <<'EOF'
sequenceDiagram
    participant Cron
    participant Report as Reorder report
    participant DB as inventory.db
    participant Buyer
    Cron->>Report: 06:00 UTC run
    Report->>DB: stock below reorder point
    DB-->>Report: rows per store
    Report->>Buyer: CSV per store
    Buyer->>DB: create purchase orders
EOF
# A chart for the image viewer: stock on hand per category, drawn pixel by
# pixel (no fonts, no libraries) and written as a PNG with zlib.
python3 - <<'PY'
import struct, zlib
W, H = 960, 540
bg, grid, axis = (246, 244, 239), (226, 222, 214), (90, 90, 90)
bars = [(0.82, (46, 110, 170)), (0.46, (224, 122, 52)), (0.64, (72, 150, 96)),
        (0.30, (196, 64, 72)), (0.71, (128, 96, 168)), (0.55, (40, 150, 160))]
px = [[bg] * W for _ in range(H)]
for y in range(60, H - 60, 70):
    for x in range(80, W - 40):
        px[y][x] = grid
for i, (v, colour) in enumerate(bars):
    x0 = 110 + i * 135
    top = int((H - 60) - v * (H - 140))
    for y in range(top, H - 60):
        for x in range(x0, x0 + 90):
            shade = 1.0 - 0.18 * (x - x0) / 90
            px[y][x] = tuple(int(c * shade) for c in colour)
for x in range(80, W - 40):
    px[H - 60][x] = axis
for y in range(50, H - 59):
    px[y][80] = axis
raw = b"".join(b"\0" + bytes(c for p in row for c in p) for row in px)
def chunk(kind, data):
    body = kind + data
    return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 2, 0, 0, 0))
png += chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
open("docs/stock-by-category.png", "wb").write(png)
PY
git add -A
commit "2026-08-18T16:22:00Z" "Add architecture diagram"

# A small binary blob for the hex viewer: a label-printer template.
python3 - <<'EOF'
import struct
header = b"LBL1" + struct.pack("<HHI", 2, 48, 0x0000BEEF)
body = bytes((i * 37 + 11) % 256 for i in range(400))
footer = b"HX-1001\x00Hex bolt M8x40\x00\x00" + struct.pack("<d", 7.90)
open("data/label-template.bin", "wb").write(header + body + footer)
EOF
git add -A
commit "2026-08-25T11:48:00Z" "Add label printer template"

mkdir -p .termide
cat > .termide/commands.toml <<'EOF'
[test]
name = "Run tests"
command = "python3 -m unittest -q"
group = "project"

[report]
name = "Reorder report (north)"
command = "python3 -m inventory.cli report --store north"
group = "project"
mode = "report"
EOF
cat > inventory/worker.py <<'PY'
"""Background worker: re-checks stock levels and queues reorder drafts."""

import sqlite3
import time
from pathlib import Path

DB = Path(__file__).resolve().parent.parent / "data" / "inventory.db"


def low_items(conn):
    return conn.execute(
        "SELECT s.store, i.sku FROM stock s JOIN items i ON i.sku = s.sku "
        "WHERE s.on_hand < i.reorder_point"
    ).fetchall()


def main():
    conn = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    while True:
        rows = low_items(conn)
        # Score every pending item; stands in for the real forecasting step.
        sum(hash((store, sku, n)) % 97 for store, sku in rows for n in range(60000))
        time.sleep(0.2)


if __name__ == "__main__":
    main()
PY
git add -A
commit "2026-09-02T08:30:00Z" "Add project commands"

# --- 3. a feature branch with its own worktree ------------------------------------
git branch feature/csv-export
git worktree add -q "$ROOT/inventory-csv-export" feature/csv-export
(
    cd "$ROOT/inventory-csv-export"
    sed -i 's/^def to_csv(rows: list\[tuple\]) -> str:/def to_csv(rows: list[tuple], delimiter: str = ",") -> str:/' inventory/report.py
    sed -i 's/    writer = csv.writer(buffer)/    writer = csv.writer(buffer, delimiter=delimiter)/' inventory/report.py
    git add -A
    commit "2026-09-10T13:15:00Z" "Allow a custom CSV delimiter"
)

sed -i 's/__version__ = "0.4.0"/__version__ = "0.4.1"/' inventory/__init__.py
git add -A
commit "2026-09-15T17:02:00Z" "Release 0.4.1"
GIT_COMMITTER_DATE="2026-09-15T17:02:00Z" git tag -a v0.4.1 -m "0.4.1"

git branch fix/stock-rounding
GIT_AUTHOR_DATE="2026-09-18T09:40:00Z" GIT_COMMITTER_DATE="2026-09-18T09:40:00Z" \
    git merge -q --no-ff feature/csv-export -m "Merge branch 'feature/csv-export'"

# --- 4. work in progress, so every git status colour shows -------------------------
cat >> inventory/models.py <<'EOF'


def low_stock(items: list[Item], ratio: float = 0.25) -> list[Item]:
    """Items at or below `ratio` of their reorder point."""
    return [item for item in items if item.on_hand <= item.reorder_point * ratio]
EOF
sed -i 's/Stock levels are counted nightly; the report runs at 06:00 UTC./Stock levels are counted nightly at 22:00; the report runs at 06:00 UTC./' README.md
git add README.md
cat > inventory/suppliers.py <<'EOF'
SUPPLIERS = {
    "Acme Supply": "orders@acme.example",
    "Colorline": "trade@colorline.example",
    "Voltworks": "b2b@voltworks.example",
}
EOF

# Fixed timestamps, so the file manager shows the same dates on every run.
# libfaketime shifts the times stat() reports by the fake clock's offset but
# leaves utimensat() alone, so stamp each file with the target minus it.
offset=$(( $(date +%s) - $(LD_PRELOAD= date +%s) ))
stamp=$(( $(date -d "2026-09-27 09:58:00" +%s) - offset ))
find "$ROOT" -path '*/.git/*' -prune -o -exec touch -h -d "@$stamp" {} +
git status -s >/dev/null  # refresh the index stat cache after the touch

# Named launchers for the demo services, so a process list reads
# inventory-api / stock-worker rather than python3 twice.
mkdir -p "$ROOT/bin"
ln -sf /usr/bin/python3 "$ROOT/bin/inventory-api"
ln -sf /usr/bin/python3 "$ROOT/bin/stock-worker"

echo "demo project ready: $PROJECT"
