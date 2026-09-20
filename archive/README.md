# archive

What this program was built from.

## `thonk.py`

The Python reimplementation that `thonkr` is a port of. It is kept here for
reference, and it is **not dead code**: the test fixtures are generated from
it, and continuous integration runs it against `thonkr` on every change to the
engine. Deleting it breaks `tests/make_fixtures.py` and
`tests/parity/parity.sh`.

It needs numpy.

```sh
python3 archive/thonk.py in.aiff out.aiff --score hectic
```

## `RUST_MIGRATION.md`

The plan the port was carried out to, in eight phases, with a record of what
changed along the way and why. Nothing reads it; it is history.
