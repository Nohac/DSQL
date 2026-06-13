# Add metadata schema version validation

**ID:** 1099a435 | **Status:** Open | **Created:** 2026-06-11T18:35:53+02:00

## Summary

Add schema versioning and validation for DSQL project metadata files.

## Context

Project metadata should declare the metadata schema version it uses. The current
CLI should check that version before consuming metadata so unsupported or stale
metadata fails with a clear diagnostic instead of being interpreted incorrectly.

Before 1.0, prefer clean metadata formats over migration code unless migration
support is explicitly needed.

## Done When

- Metadata files include a schema version field.
- The CLI validates the schema version before using metadata.
- Unsupported versions produce clear errors.
- Tests cover current, missing, and unsupported schema versions.
