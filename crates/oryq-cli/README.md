# `oryq-cli` (`oryq`)

Zero-extra-dependency command-line utility for OryqDB built exclusively over `oryq-core`:

- Database inspection: `info`, `collections`, `inspect <db> page <id> | wal | locks`
- CRUD & Pipelines: `create-collection`, `drop-collection`, `insert`, `find`, `update`, `delete`, `aggregate`
- Maintenance: `checkpoint`, `backup`, `integrity-check`
