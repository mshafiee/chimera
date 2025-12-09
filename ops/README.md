# Operations Scripts

## Roster Merge Scripts

### Python Script (Recommended)

The `merge_roster.py` script can merge wallets from `roster_new.db` into the main `chimera.db` database.

**Usage:**

```bash
# From host machine
docker compose --profile devnet cp ops/merge_roster.py scout:/tmp/merge_roster.py
docker compose --profile devnet exec scout python /tmp/merge_roster.py

# Or with custom paths
docker compose --profile devnet exec scout python /tmp/merge_roster.py \
  /app/data/roster_new.db \
  /app/data/chimera.db
```

**What it does:**
1. Checks roster file integrity
2. Counts wallets in roster
3. Deletes existing wallets from main database
4. Inserts all wallets from roster_new.db
5. Reports merge statistics

### Shell Scripts

#### `merge-roster.sh` (Full-featured)

Tries API endpoint first, falls back to direct SQLite merge if available.

**Usage:**
```bash
# From host
./ops/merge-roster.sh

# Inside container
docker compose exec operator sh < ops/merge-roster.sh
```

#### `merge-roster-simple.sh` (Minimal)

Simple script that tries API first, then direct SQLite.

**Usage:**
```bash
cat ops/merge-roster-simple.sh | docker compose exec operator sh
```

### Rust Binary (Future)

A Rust binary `merge_roster` can be built and used:

```bash
cd operator
cargo build --bin merge_roster --release
./target/release/merge_roster --roster-path data/roster_new.db --db-path data/chimera.db
```

## When to Use

- **After Scout runs**: Scout analyzes wallets and writes to `roster_new.db`, but doesn't automatically merge
- **Manual updates**: When you want to manually update the wallet roster
- **Testing**: During development to test roster merge functionality

## Notes

- The merge **replaces** all existing wallets (doesn't merge/update individual records)
- Always backup your database before merging in production
- The roster file must pass integrity checks before merge
- In devnet, the API endpoint may not require authentication (if configured)
