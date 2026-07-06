#!/usr/bin/env python3
"""
Schema generator for Chimera database.
Reads YAML schemas from database/schema_yaml/ and generates:
- operator/migrations/0001_full_schema.sql (SQLite dialect)
- operator/migrations_postgres/0001_full_schema.sql (PostgreSQL dialect)
- scout/schema_scout_tables.sql (PG dialect for scout to apply at startup)
- scout/schema_scout_tables_sqlite.sql (SQLite variant for scout)
"""

import os
import sys
import glob
import yaml
from pathlib import Path
from typing import Dict, List, Any


# Type mapping
TYPE_MAP = {
    'text': {'sqlite': 'TEXT', 'postgres': 'TEXT'},
    'bool': {'sqlite': 'INTEGER', 'postgres': 'BOOLEAN'},
    'int': {'sqlite': 'INTEGER', 'postgres': 'INTEGER'},
    'bigint': {'sqlite': 'INTEGER', 'postgres': 'BIGINT'},
    'bigserial': {'sqlite': 'INTEGER PRIMARY KEY AUTOINCREMENT', 'postgres': 'BIGSERIAL PRIMARY KEY'},
    'real': {'sqlite': 'REAL', 'postgres': 'DOUBLE PRECISION'},
    'decimal': {'sqlite': 'TEXT', 'postgres': 'NUMERIC'},  # Will handle precision separately
    'timestamp': {'sqlite': 'TIMESTAMP', 'postgres': 'TIMESTAMPTZ'},
    'json': {'sqlite': 'TEXT', 'postgres': 'JSONB'},
    'blob': {'sqlite': 'BLOB', 'postgres': 'BYTEA'},
}


def parse_type(type_str: str) -> tuple:
    """Parse type string like 'decimal(30,18)' into (base_type, precision, scale)"""
    if '(' in type_str:
        base, rest = type_str.split('(', 1)
        rest = rest.rstrip(')')
        parts = rest.split(',')
        if len(parts) == 2:
            return base, int(parts[0]), int(parts[1])
        elif len(parts) == 1:
            return base, int(parts[0]), None
    return type_str, None, None


def map_type(type_str: str, dialect: str) -> str:
    """Map YAML type to dialect-specific SQL type"""
    base_type, precision, scale = parse_type(type_str)
    
    if base_type not in TYPE_MAP:
        raise ValueError(f"Unknown type: {base_type}")
    
    mapped = TYPE_MAP[base_type][dialect]
    
    # Handle precision for decimal types
    if base_type == 'decimal':
        if dialect == 'sqlite':
            # SQLite stores decimals as TEXT without precision spec
            return 'TEXT'
        elif precision is not None:
            if scale is not None:
                return f"{mapped}({precision},{scale})"
            else:
                return f"{mapped}({precision})"
    
    return mapped


def load_schemas() -> Dict[str, Dict]:
    """Load all YAML schema files"""
    schemas = {}
    yaml_dir = Path(__file__).parent.parent / 'database' / 'schema_yaml'
    
    if not yaml_dir.exists():
        raise FileNotFoundError(f"Schema directory not found: {yaml_dir}")
    
    for yaml_file in sorted(yaml_dir.glob('*.yaml')):
        with open(yaml_file) as f:
            schema = yaml.safe_load(f)
            table_name = schema['table']
            schemas[table_name] = schema
    
    return schemas


def format_default(default: Any, dialect: str) -> str:
    """Format default value for dialect"""
    if default is None:
        return ''
    
    # Convert to string if not already
    if not isinstance(default, str):
        if isinstance(default, bool):
            return 'TRUE' if default else 'FALSE' if dialect == 'postgres' else ('1' if default else '0')
        elif isinstance(default, (int, float)):
            return str(default)
        else:
            default = str(default)
    
    # Remove quotes if they're already there
    default = default.strip()
    if default.startswith("'") and default.endswith("'"):
        default = default[1:-1]
    
    # Handle special values
    if default.upper() in ('TRUE', 'FALSE'):
        return default.upper() if dialect == 'postgres' else ('1' if default.upper() == 'TRUE' else '0')
    if default.upper() == 'CURRENT_TIMESTAMP':
        return default if dialect == 'postgres' else "CURRENT_TIMESTAMP"
    if default.upper() == 'NULL':
        return 'NULL'
    
    # String literal - need to quote
    return f"'{default}'"


def generate_column_def(column: Dict, dialect: str) -> str:
    """Generate column definition SQL"""
    parts = []
    
    # Column name
    parts.append(f"    {column['name']}")
    
    # Type
    parts.append(map_type(column['type'], dialect))
    
    # Primary key
    if column.get('pk', False):
        if dialect == 'sqlite' and column['type'] == 'bigserial':
            # Already handled in type mapping
            pass
        elif dialect == 'postgres' and column['type'] == 'bigserial':
            # Already handled in type mapping
            pass
        else:
            parts.append('PRIMARY KEY')
    
    # Not null
    if column.get('not_null', False):
        parts.append('NOT NULL')
    
    # Unique
    if column.get('unique', False):
        parts.append('UNIQUE')
    
    # Default
    if 'default' in column:
        parts.append(f"DEFAULT {format_default(column['default'], dialect)}")
    
    # Check constraint
    if 'check' in column:
        parts.append(f"CHECK({column['check']})")
    
    return ' '.join(parts)


def generate_create_table(table_name: str, schema: Dict, dialect: str, if_not_exists: bool = True) -> str:
    """Generate CREATE TABLE statement"""
    lines = []
    
    # Table description comment
    if 'description' in schema and dialect == 'postgres':
        lines.append(f"-- {schema['description']}")
        lines.append(f"COMMENT ON TABLE {table_name} IS '{schema['description']}';")
    
    # CREATE TABLE
    if_exists = 'IF NOT EXISTS ' if if_not_exists else ''
    lines.append(f"CREATE TABLE {if_exists}{table_name} (")
    
    # Columns
    column_defs = []
    for col in schema['columns']:
        column_defs.append(generate_column_def(col, dialect) + ',')
    
    # Foreign keys
    for fk in schema.get('foreign_keys', []):
        on_delete = fk.get('on_delete', 'NO ACTION')
        fk_def = f"    FOREIGN KEY ({fk['column']}) REFERENCES {fk['ref_table']}({fk['ref_col']}) ON DELETE {on_delete}"
        column_defs.append(fk_def + ',')
    
    # Table-level check constraints
    for col in schema['columns']:
        if 'check' in col and 'pk' in col and col['pk']:
            # Already handled in column definition
            continue
        if 'check' in col and not col.get('pk', False):
            # Already handled in column definition
            continue
    
    # Remove trailing comma from last column definition
    if column_defs:
        column_defs[-1] = column_defs[-1].rstrip(',')
    
    lines.extend(column_defs)
    lines.append(');')
    
    return '\n'.join(lines)


def generate_indexes(table_name: str, schema: Dict, dialect: str) -> str:
    """Generate index statements"""
    lines = []
    
    for idx in schema.get('indexes', []):
        # Skip Postgres-only indexes for SQLite
        if idx.get('pg_only', False) and dialect == 'sqlite':
            continue
        
        index_name = idx['name']
        
        # Handle different column formats - try 'columns' first, then 'on'
        columns = idx.get('columns', idx.get('on', []))
        if not isinstance(columns, list):
            columns = [columns]
        
        where_clause = idx.get('where', '')
        unique = 'UNIQUE ' if idx.get('unique', False) else ''
        desc = ' DESC' if idx.get('desc', False) else ''
        index_type = ''
        
        # Handle index types
        if 'type' in idx:
            if idx['type'] == 'gin' and dialect == 'postgres':
                index_type = f"USING GIN "
            elif idx['type'] == 'brin' and dialect == 'postgres':
                index_type = f"USING BRIN "
            elif idx['type'] in ('gin', 'brin') and dialect == 'sqlite':
                # Skip GIN/BRIN indexes for SQLite
                continue
        
        # Format column list
        col_list = ', '.join([f"{col}{desc}" if i == len(columns) - 1 else str(col) for i, col in enumerate(columns)])
        
        # Build index statement
        where_sql = f" WHERE {where_clause}" if where_clause else ''
        lines.append(f"CREATE {unique}INDEX IF NOT EXISTS {index_name} ON {table_name} {index_type}({col_list}){where_sql};")
    
    return '\n'.join(lines)


def generate_triggers(table_name: str, schema: Dict, dialect: str) -> str:
    """Generate trigger statements (PostgreSQL only)"""
    if dialect == 'sqlite':
        return ''
    
    lines = []
    
    for trigger in schema.get('triggers', []):
        col_name = trigger['column']
        trigger_name = trigger['name']
        func_name = f"update_{col_name}_column"
        
        lines.append(f"CREATE TRIGGER {trigger_name}")
        lines.append(f"    BEFORE UPDATE ON {table_name}")
        lines.append(f"    FOR EACH ROW")
        lines.append(f"    EXECUTE FUNCTION {func_name}();")
    
    return '\n'.join(lines)


def generate_seed_data(table_name: str, schema: Dict, dialect: str) -> str:
    """Generate seed data INSERT statements"""
    if 'seed' not in schema:
        return ''
    
    lines = []
    
    for seed_row in schema['seed']:
        columns = list(seed_row.keys())
        values = list(seed_row.values())
        
        # Format values
        formatted_values = []
        for val in values:
            if isinstance(val, str):
                # Remove extra quotes if present
                val = val.strip().strip("'")
                formatted_values.append(f"'{val}'")
            elif val is None:
                formatted_values.append('NULL')
            elif isinstance(val, bool):
                formatted_values.append('TRUE' if val else 'FALSE')
            else:
                formatted_values.append(str(val))
        
        col_list = ', '.join(columns)
        val_list = ', '.join(formatted_values)
        
        if dialect == 'sqlite':
            lines.append(f"INSERT OR IGNORE INTO {table_name} ({col_list}) VALUES ({val_list});")
        else:
            # For PostgreSQL, need to handle ON CONFLICT for each row
            pk_col = columns[0]  # Assume first column is PK
            lines.append(f"INSERT INTO {table_name} ({col_list}) VALUES ({val_list})")
            lines.append(f"ON CONFLICT ({pk_col}) DO NOTHING;")
    
    return '\n'.join(lines)


def generate_pg_functions() -> str:
    """Generate PostgreSQL trigger functions"""
    return """
-- =============================================================================
-- FUNCTIONS (PostgreSQL triggers require functions)
-- =============================================================================

-- Generic updated_at trigger function
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = CURRENT_TIMESTAMP;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Generic last_updated trigger function
CREATE OR REPLACE FUNCTION update_last_updated_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.last_updated = CURRENT_TIMESTAMP;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
"""


def generate_migration(schemas: Dict[str, Dict], dialect: str, scout_tables_only: bool = False) -> str:
    """Generate complete migration SQL"""
    lines = []
    
    # Header
    if dialect == 'postgres':
        lines.append("-- Chimera Database Schema - PostgreSQL")
        lines.append("-- Generated from database/schema_yaml/*.yaml")
        lines.append("")
        lines.append("-- Enable required extensions")
        lines.append("CREATE EXTENSION IF NOT EXISTS \"uuid-ossp\";")
        lines.append("CREATE EXTENSION IF NOT EXISTS \"pg_trgm\";")
        lines.append("")
        
        # Add trigger functions
        if not scout_tables_only:
            lines.append(generate_pg_functions())
            lines.append("")
    else:
        lines.append("-- Chimera Database Schema - SQLite")
        lines.append("-- Generated from database/schema_yaml/*.yaml")
        lines.append("-- Financial values stored as TEXT (Decimal strings) to avoid IEEE 754 precision loss")
        lines.append("")
    
    # Determine which tables to include
    if scout_tables_only:
        # Scout-owned tables
        scout_tables = [
            'ml_predictions', 'exit_recommendations', 'alerts', 'metrics', 'health_checks',
            'growth_history', 'capital_events', 'growth_alerts', 'credit_history',
            'wallet_performance_history', 'roi_metrics', 'multi_timeframe_discovery_stats'
        ]
        tables_to_generate = [(name, schemas[name]) for name in scout_tables if name in schemas]
    else:
        # Operator-owned tables
        operator_tables = [
            'schema_migrations', 'trades', 'positions', 'wallets', 'dead_letter_queue',
            'config_audit', 'kill_switch_state', 'circuit_breaker_state', 'admin_wallets',
            'jito_tip_history', 'reconciliation_log', 'backups', 'historical_liquidity',
            'wallet_monitoring', 'exit_targets', 'signal_aggregation', 'wallet_copy_performance',
            'rate_limit_metrics', 'webhook_lifecycle_audit', 'webhook_configuration',
            'wqs_pnl_correlation'
        ]
        tables_to_generate = [(name, schemas[name]) for name in operator_tables if name in schemas]
    
    # Generate table definitions
    for table_name, schema in tables_to_generate:
        lines.append(f"-- =============================================================================")
        lines.append(f"-- {schema.get('description', table_name.upper())}")
        lines.append(f"-- =============================================================================")
        lines.append("")
        
        lines.append(generate_create_table(table_name, schema, dialect))
        lines.append("")
        
        # Indexes
        indexes = generate_indexes(table_name, schema, dialect)
        if indexes:
            lines.append(indexes)
            lines.append("")
        
        # Seed data
        seed = generate_seed_data(table_name, schema, dialect)
        if seed:
            lines.append(seed)
            lines.append("")
        
        # Triggers (PostgreSQL only)
        triggers = generate_triggers(table_name, schema, dialect)
        if triggers:
            lines.append(triggers)
            lines.append("")
    
    return '\n'.join(lines)


def write_migration(sql: str, output_path: Path):
    """Write migration SQL to file"""
    output_path.parent.mkdir(parents=True, exist_ok=True)
    
    with open(output_path, 'w') as f:
        f.write(sql)
    
    print(f"Generated: {output_path}")


def main():
    """Main entry point"""
    print("Loading schema definitions...")
    schemas = load_schemas()
    print(f"Found {len(schemas)} table definitions")
    
    base_dir = Path(__file__).parent.parent
    
    # Generate SQLite migration for operator
    print("\nGenerating SQLite migration for operator...")
    sqlite_sql = generate_migration(schemas, 'sqlite', scout_tables_only=False)
    sqlite_path = base_dir / 'operator' / 'migrations' / '0001_full_schema.sql'
    write_migration(sqlite_sql, sqlite_path)
    
    # Generate PostgreSQL migration for operator
    print("\nGenerating PostgreSQL migration for operator...")
    pg_sql = generate_migration(schemas, 'postgres', scout_tables_only=False)
    pg_path = base_dir / 'operator' / 'migrations_postgres' / '0001_full_schema.sql'
    write_migration(pg_sql, pg_path)
    
    # Generate PostgreSQL schema for scout
    print("\nGenerating PostgreSQL schema for scout...")
    scout_pg_sql = generate_migration(schemas, 'postgres', scout_tables_only=True)
    scout_pg_path = base_dir / 'scout' / 'schema_scout_tables.sql'
    write_migration(scout_pg_sql, scout_pg_path)
    
    # Generate SQLite schema for scout
    print("\nGenerating SQLite schema for scout...")
    scout_sqlite_sql = generate_migration(schemas, 'sqlite', scout_tables_only=True)
    scout_sqlite_path = base_dir / 'scout' / 'schema_scout_tables_sqlite.sql'
    write_migration(scout_sqlite_sql, scout_sqlite_path)
    
    print("\n✅ All schema files generated successfully!")


if __name__ == '__main__':
    main()