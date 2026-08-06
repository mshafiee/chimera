-- Fix btree_gin extension schema reference.
-- In this database the public schema has a non-standard OID (28081), but the
-- btree_gin extension was registered with extnamespace = 2200 (the default
-- public OID), which does not exist here. This made pg_dump fail with
-- "schema with OID 2200 does not exist", breaking the automated backups.

ALTER EXTENSION btree_gin SET SCHEMA public;
