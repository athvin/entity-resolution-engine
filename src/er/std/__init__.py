"""Python oracles for the S4.2 standardization macros (S4.2, S6).

S3's tree names no directory for them, because in v1 every standardization rule
that has one implementation has it in `dbt/macros/std/`. `address_parse` is the
exception S4.2 writes down: it is a componentizer *behind an interface*, versioned
by `versions.address_parser_version`, so the version has to select something a
Python process can hold. This package is that something. A layout amendment is a
spec ticket; until it lands, `tests/unit/test_package_layout.py` allows this one
directory by name rather than the module moving somewhere it does not belong.
"""
