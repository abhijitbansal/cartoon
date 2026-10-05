"""Roman numeral conversion for 1..3999."""

_NUMERALS = [
    (1000, "M"),
    (900, "CM"),
    (500, "D"),
    (400, "CD"),
    (100, "C"),
    (90, "XC"),
    (50, "L"),
    (40, "XL"),
    (10, "X"),
    (9, "IX"),
    (5, "V"),
    (4, "IV"),
    (1, "I"),
]

_VALUES = {"I": 1, "V": 5, "X": 10, "L": 50, "C": 100, "D": 500, "M": 1000}


def to_roman(n: int) -> str:
    """Return the canonical Roman numeral for ``n`` (1..3999)."""
    if not isinstance(n, int) or isinstance(n, bool):
        raise TypeError("expected an int")
    if not 0 < n < 4000:
        raise ValueError(f"out of range: {n}")
    out = []
    for value, symbol in _NUMERALS:
        count, n = divmod(n, value)
        out.append(symbol * count)
    return "".join(out)


def from_roman(s: str) -> int:
    """Parse a Roman numeral (subtractive notation allowed)."""
    if not s:
        raise ValueError("empty numeral")
    total = 0
    prev = 0
    for ch in reversed(s.upper()):
        if ch not in _VALUES:
            raise ValueError(f"not a numeral: {s!r}")
        value = _VALUES[ch]
        if value < prev:
            total -= value
        else:
            total += value
            prev = value
    return total
