import re

import pytest

from roman.convert import from_roman, to_roman

KNOWN = {
    1: "I",
    3: "III",
    4: "IV",
    9: "IX",
    14: "XIV",
    40: "XL",
    44: "XLIV",
    90: "XC",
    99: "XCIX",
    400: "CD",
    444: "CDXLIV",
    500: "D",
    900: "CM",
    944: "CMXLIV",
    999: "CMXCIX",
    1066: "MLXVI",
    1666: "MDCLXVI",
    1904: "MCMIV",
    1954: "MCMLIV",
    1990: "MCMXC",
    1994: "MCMXCIV",
    2014: "MMXIV",
    2421: "MMCDXXI",
    3999: "MMMCMXCIX",
}

# The only valid (canonical) spellings of 1..3999.
CANONICAL = re.compile(r"^M{0,3}(CM|CD|D?C{0,3})(XC|XL|L?X{0,3})(IX|IV|V?I{0,3})$")


@pytest.mark.parametrize("n,numeral", sorted(KNOWN.items()))
def test_known_values(n, numeral):
    assert to_roman(n) == numeral


@pytest.mark.parametrize("n", range(1, 4000, 50))
def test_roundtrip(n):
    assert from_roman(to_roman(n)) == n


@pytest.mark.parametrize("n", range(7, 4000, 50))
def test_canonical_form(n):
    numeral = to_roman(n)
    assert CANONICAL.match(numeral), f"{n} -> {numeral} is not canonical"


@pytest.mark.parametrize("bad", [0, -1, 4000, 10**6])
def test_out_of_range(bad):
    with pytest.raises(ValueError):
        to_roman(bad)


@pytest.mark.parametrize("bad", ["", "ABC", "MX7"])
def test_from_roman_rejects_garbage(bad):
    with pytest.raises(ValueError):
        from_roman(bad)
