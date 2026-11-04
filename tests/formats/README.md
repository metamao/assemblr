# Format samples

One sample of every persisted format that Assemblr promises to read
([ADR 0041](../../docs/adr/0041-format-compatibility.md),
[compatibility](../../docs/domain/compatibility.md)). Unit tests next to each
reader read these files back, so a change that stops reading a format fails a
test.

Never edit or delete a sample. A new format, or a new form of an authored
file, adds a sample beside the earlier ones.
