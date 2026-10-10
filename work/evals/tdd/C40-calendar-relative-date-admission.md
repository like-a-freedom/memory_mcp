# C40 calendar relative-date admission

The admission tests use a natural question and source-shaped fact with a fixed reference date of 2023-04-01. “Mara just bought a bicycle yesterday” and “Mara just bought a bicycle last week” should match March, not April.

RED: `rtk proxy cargo test -p memory_mcp --locked --lib calendar_month_ -- --nocapture` initially failed both boundary assertions: calendar admission treated the record reference date as the event date. After adding the `just` examples, four assertions failed because the vague token masked the explicit temporal phrase.

GREEN: the same command passed seven tests after calendar admission reused the relative-date-aware sentence interpreter, ignored `just` as a date, and rejected contradictory relative markers.
